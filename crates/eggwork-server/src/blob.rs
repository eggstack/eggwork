//! Node-owned content-addressed blob storage.

use bytes::Bytes;
use eggwork_core::BlobDigest;
use futures_util::{Stream, StreamExt};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use tokio::{io::AsyncWriteExt, sync::Mutex};

pub const MAX_BLOB_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_FIND_DIGESTS: usize = 512;

#[derive(Debug, Error)]
pub enum BlobError {
    #[error("blob is too large")]
    TooLarge,
    #[error("blob storage quota exceeded")]
    QuotaExceeded,
    #[error("blob length does not match the declared length")]
    LengthMismatch,
    #[error("blob content does not match its digest")]
    DigestMismatch,
    #[error("stored blob is corrupt")]
    CorruptExisting,
    #[error("blob does not exist")]
    NotFound,
    #[error("too many blob digests in one request")]
    TooManyDigests,
    #[error("blob storage operation failed")]
    Io(#[from] io::Error),
    #[error("blob metadata operation failed")]
    Metadata(#[from] rusqlite::Error),
    #[error("blob body stream failed")]
    Body,
    #[error("blob worker failed")]
    Worker,
}

#[derive(Clone)]
pub struct BlobStore {
    inner: Arc<BlobStoreInner>,
}

struct BlobStoreInner {
    root: PathBuf,
    quota_bytes: u64,
    metadata: std::sync::Mutex<Connection>,
    write_lock: Mutex<()>,
}

impl BlobStore {
    pub fn open(root: impl AsRef<Path>, quota_bytes: u64) -> Result<Self, BlobError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        }
        let quarantine = root.join("quarantine");
        fs::create_dir_all(&quarantine)?;
        let metadata = Connection::open(root.join("metadata.sqlite"))?;
        metadata.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             CREATE TABLE IF NOT EXISTS blobs (
               digest TEXT PRIMARY KEY,
               size_bytes INTEGER NOT NULL CHECK(size_bytes >= 0),
               reference_count INTEGER NOT NULL DEFAULT 0 CHECK(reference_count >= 0),
               last_used_unix_ms INTEGER NOT NULL
             );",
        )?;
        let store = Self {
            inner: Arc::new(BlobStoreInner {
                root,
                quota_bytes,
                metadata: std::sync::Mutex::new(metadata),
                write_lock: Mutex::new(()),
            }),
        };
        store.remove_stale_parts()?;
        Ok(store)
    }

    pub fn path_for(&self, digest: &BlobDigest) -> PathBuf {
        self.inner
            .root
            .join(&digest.as_str()[..2])
            .join(digest.as_str())
    }

    pub fn find_missing(&self, digests: &[BlobDigest]) -> Result<Vec<BlobDigest>, BlobError> {
        if digests.len() > MAX_FIND_DIGESTS {
            return Err(BlobError::TooManyDigests);
        }
        let connection = self.inner.metadata.lock().map_err(|_| BlobError::Worker)?;
        let mut present = HashSet::new();
        let mut statement = connection.prepare("SELECT digest FROM blobs WHERE digest = ?1")?;
        for digest in digests {
            if statement
                .query_row([digest.as_str()], |row| row.get::<_, String>(0))
                .optional()?
                .is_some()
            {
                present.insert(digest.as_str().to_owned());
            }
        }
        Ok(digests
            .iter()
            .filter(|digest| !present.contains(digest.as_str()))
            .cloned()
            .collect())
    }

    pub async fn put_stream<S, E>(
        &self,
        digest: BlobDigest,
        declared_length: u64,
        input: S,
    ) -> Result<(), BlobError>
    where
        S: Stream<Item = Result<Bytes, E>>,
    {
        if declared_length > MAX_BLOB_BYTES {
            return Err(BlobError::TooLarge);
        }
        let _guard = self.inner.write_lock.lock().await;
        let target = self.path_for(&digest);
        if target.exists() {
            return self.verify_existing(&digest).await;
        }
        let used: i64 = self
            .inner
            .metadata
            .lock()
            .map_err(|_| BlobError::Worker)?
            .query_row(
                "SELECT COALESCE(SUM(size_bytes), 0) FROM blobs",
                [],
                |row| row.get(0),
            )?;
        if used as u64 + declared_length > self.inner.quota_bytes {
            return Err(BlobError::QuotaExceeded);
        }

        let parent = target.parent().ok_or(BlobError::Worker)?;
        fs::create_dir_all(parent)?;
        let part = self
            .inner
            .root
            .join(format!(".part-{}", uuid::Uuid::new_v4()));
        let std_file = private_create_new(&part)?;
        let mut file = tokio::fs::File::from_std(std_file);
        let result = async {
            let mut input = Box::pin(input);
            let mut size = 0u64;
            let mut hasher = Sha256::new();
            while let Some(chunk) = input.next().await {
                let chunk = chunk.map_err(|_| BlobError::Body)?;
                size = size
                    .checked_add(chunk.len() as u64)
                    .ok_or(BlobError::TooLarge)?;
                if size > MAX_BLOB_BYTES || size > declared_length {
                    return Err(BlobError::TooLarge);
                }
                hasher.update(&chunk);
                file.write_all(&chunk).await?;
            }
            if size != declared_length {
                return Err(BlobError::LengthMismatch);
            }
            if hex::encode(hasher.finalize()) != digest.as_str() {
                return Err(BlobError::DigestMismatch);
            }
            file.sync_all().await?;
            drop(file);
            fs::rename(&part, &target)?;
            sync_directory(parent)?;
            let connection = self.inner.metadata.lock().map_err(|_| BlobError::Worker)?;
            connection.execute(
                "INSERT INTO blobs(digest, size_bytes, reference_count, last_used_unix_ms)
                 VALUES (?1, ?2, 0, ?3)",
                params![digest.as_str(), size as i64, unix_millis()],
            )?;
            Ok::<(), BlobError>(())
        }
        .await;
        if result.is_err() {
            let _ = fs::remove_file(&part);
            // A failed metadata commit must not leave a readable unindexed blob.
            if target.exists()
                && self
                    .inner
                    .metadata
                    .lock()
                    .ok()
                    .and_then(|connection| {
                        connection
                            .query_row(
                                "SELECT 1 FROM blobs WHERE digest = ?1",
                                [digest.as_str()],
                                |_| Ok(()),
                            )
                            .optional()
                            .ok()
                            .flatten()
                    })
                    .is_none()
            {
                let _ = fs::remove_file(&target);
            }
        }
        result
    }

    pub async fn verify_existing(&self, digest: &BlobDigest) -> Result<(), BlobError> {
        let path = self.path_for(digest);
        let info = self
            .inner
            .metadata
            .lock()
            .map_err(|_| BlobError::Worker)?
            .query_row(
                "SELECT size_bytes FROM blobs WHERE digest = ?1",
                [digest.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        let Some(expected_size) = info else {
            return Err(BlobError::NotFound);
        };
        let digest_for_hash = digest.clone();
        let corrupt = tokio::task::spawn_blocking(move || {
            let mut file = fs::File::open(&path)?;
            let mut writer = HashWriter {
                hasher: Sha256::new(),
                written: 0,
            };
            io::copy(&mut file, &mut writer)?;
            Ok::<_, io::Error>(
                writer.written != expected_size as u64
                    || hex::encode(writer.hasher.finalize()) != digest_for_hash.as_str(),
            )
        })
        .await
        .map_err(|_| BlobError::Worker)??;
        if corrupt {
            let path = self.path_for(digest);
            let quarantine = self.inner.root.join("quarantine").join(format!(
                "{}-{}",
                digest.as_str(),
                uuid::Uuid::new_v4()
            ));
            fs::rename(path, quarantine)?;
            self.inner
                .metadata
                .lock()
                .map_err(|_| BlobError::Worker)?
                .execute("DELETE FROM blobs WHERE digest = ?1", [digest.as_str()])?;
            return Err(BlobError::CorruptExisting);
        }
        self.inner
            .metadata
            .lock()
            .map_err(|_| BlobError::Worker)?
            .execute(
                "UPDATE blobs SET last_used_unix_ms = ?2 WHERE digest = ?1",
                params![digest.as_str(), unix_millis()],
            )?;
        Ok(())
    }

    pub async fn open_verified(&self, digest: &BlobDigest) -> Result<tokio::fs::File, BlobError> {
        self.verify_existing(digest).await?;
        Ok(tokio::fs::File::open(self.path_for(digest)).await?)
    }

    fn remove_stale_parts(&self) -> Result<(), BlobError> {
        for entry in fs::read_dir(&self.inner.root)? {
            let entry = entry?;
            if entry.file_name().to_string_lossy().starts_with(".part-") {
                let metadata = entry.metadata()?;
                if metadata.is_file() {
                    fs::remove_file(entry.path())?;
                }
            }
        }
        let known: HashSet<String> = {
            let connection = self.inner.metadata.lock().map_err(|_| BlobError::Worker)?;
            let mut statement = connection.prepare("SELECT digest FROM blobs")?;
            let rows = statement.query_map([], |row| row.get(0))?;
            rows.collect::<Result<_, _>>()?
        };
        // Reconcile the two durable layers after a crash between rename and
        // metadata commit (or metadata commit and external file loss).
        for digest in &known {
            if BlobDigest::parse(digest.clone()).is_err() {
                self.inner
                    .metadata
                    .lock()
                    .map_err(|_| BlobError::Worker)?
                    .execute("DELETE FROM blobs WHERE digest = ?1", [digest])?;
                continue;
            }
            let path = self.inner.root.join(&digest[..2]).join(digest);
            if !path.is_file() {
                self.inner
                    .metadata
                    .lock()
                    .map_err(|_| BlobError::Worker)?
                    .execute("DELETE FROM blobs WHERE digest = ?1", [digest])?;
            }
        }
        for directory in fs::read_dir(&self.inner.root)? {
            let directory = directory?;
            if !directory.file_type()?.is_dir() || directory.file_name() == "quarantine" {
                continue;
            }
            for file in fs::read_dir(directory.path())? {
                let file = file?;
                if !file.file_type()?.is_file() {
                    continue;
                }
                let name = file.file_name().to_string_lossy().into_owned();
                if BlobDigest::parse(name.clone()).is_ok() && !known.contains(&name) {
                    fs::remove_file(file.path())?;
                }
            }
        }
        Ok(())
    }
}

struct HashWriter {
    hasher: Sha256,
    written: u64,
}
impl io::Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.hasher.update(bytes);
        self.written = self
            .written
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| io::Error::other("blob size overflow"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn private_create_new(path: &Path) -> io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    fs::File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{future::join_all, stream};
    use tempfile::TempDir;

    #[tokio::test]
    async fn empty_blob_and_digest_path_are_canonical() {
        let temp = TempDir::new().unwrap();
        let store = BlobStore::open(temp.path().join("blobs"), 1024).unwrap();
        let empty = BlobDigest::from_bytes(b"");
        store
            .put_stream(empty.clone(), 0, stream::empty::<Result<Bytes, ()>>())
            .await
            .unwrap();
        assert!(
            store
                .find_missing(std::slice::from_ref(&empty))
                .unwrap()
                .is_empty()
        );
        assert_eq!(tokio::fs::read(store.path_for(&empty)).await.unwrap(), b"");
        assert!(BlobDigest::parse("../secret".to_owned()).is_err());
        assert!(BlobDigest::parse("A".repeat(64)).is_err());
    }

    #[tokio::test]
    async fn invalid_uploads_leave_no_readable_blob_or_partial_file() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("blobs");
        let store = BlobStore::open(&root, 1024).unwrap();
        let expected = BlobDigest::from_bytes(b"right");
        assert!(matches!(
            store
                .put_stream(
                    expected.clone(),
                    5,
                    stream::iter([Ok::<_, ()>(Bytes::from_static(b"wrong"))]),
                )
                .await,
            Err(BlobError::DigestMismatch)
        ));
        assert!(matches!(
            store
                .put_stream(
                    expected,
                    7,
                    stream::iter([Ok::<_, ()>(Bytes::from_static(b"right"))]),
                )
                .await,
            Err(BlobError::LengthMismatch)
        ));
        assert!(fs::read_dir(&root).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".part-")
        }));
    }

    #[tokio::test]
    async fn interrupted_upload_is_cleaned_and_duplicate_writers_deduplicate() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("blobs");
        let store = BlobStore::open(&root, 64).unwrap();
        let partial_digest = BlobDigest::from_bytes(b"complete");
        let broken = stream::iter([
            Ok::<_, BlobError>(Bytes::from_static(b"part")),
            Err(BlobError::Body),
        ]);
        assert!(matches!(
            store.put_stream(partial_digest.clone(), 8, broken).await,
            Err(BlobError::Body)
        ));
        assert!(store.find_missing(&[partial_digest]).unwrap().len() == 1);
        assert!(fs::read_dir(&root).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".part-")
        }));

        let bytes = Bytes::from_static(b"deduplicate");
        let digest = BlobDigest::from_bytes(&bytes);
        let tasks = (0..12).map(|_| {
            let store = store.clone();
            let bytes = bytes.clone();
            let digest = digest.clone();
            async move {
                store
                    .put_stream(
                        digest,
                        bytes.len() as u64,
                        stream::iter([Ok::<_, ()>(bytes)]),
                    )
                    .await
            }
        });
        for result in join_all(tasks).await {
            result.unwrap();
        }
        assert_eq!(fs::read(store.path_for(&digest)).unwrap(), b"deduplicate");
    }

    #[tokio::test]
    async fn quota_corruption_and_startup_cleanup_are_deterministic() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("blobs");
        let store = BlobStore::open(&root, 5).unwrap();
        let first = BlobDigest::from_bytes(b"12345");
        store
            .put_stream(
                first.clone(),
                5,
                stream::iter([Ok::<_, ()>(Bytes::from_static(b"12345"))]),
            )
            .await
            .unwrap();
        let extra = BlobDigest::from_bytes(b"x");
        assert!(matches!(
            store
                .put_stream(
                    extra,
                    1,
                    stream::iter([Ok::<_, ()>(Bytes::from_static(b"x"))]),
                )
                .await,
            Err(BlobError::QuotaExceeded)
        ));
        fs::write(store.path_for(&first), b"corrupt").unwrap();
        assert!(matches!(
            store.verify_existing(&first).await,
            Err(BlobError::CorruptExisting)
        ));
        assert!(!store.path_for(&first).exists());
        fs::write(root.join(".part-crash-fixture"), b"partial").unwrap();
        drop(store);
        let restarted = BlobStore::open(&root, 5).unwrap();
        assert!(!root.join(".part-crash-fixture").exists());
        assert_eq!(restarted.find_missing(&[first]).unwrap().len(), 1);
    }
}
