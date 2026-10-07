//! Node-owned content-addressed blob storage.

use bytes::Bytes;
use eggwork_core::BlobDigest;
use futures_util::{Stream, StreamExt};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
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
/// Digests per `IN (...)` clause in `find_missing`. Well under SQLite's
/// `SQLITE_MAX_VARIABLE_NUMBER`, which is 32766 in every build this ships on.
const FIND_CHUNK: usize = 256;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GcReport {
    pub expired_references: u64,
    pub expired_references_removed: u64,
    pub candidate_blobs: u64,
    pub candidate_bytes: u64,
    pub removed_blobs: u64,
    pub removed_bytes: u64,
}

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
             PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS blobs (
               digest TEXT PRIMARY KEY,
               size_bytes INTEGER NOT NULL CHECK(size_bytes >= 0),
               reference_count INTEGER NOT NULL DEFAULT 0 CHECK(reference_count >= 0),
               last_used_unix_ms INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS blob_references (
               owner_kind TEXT NOT NULL,
               owner_id TEXT NOT NULL,
               digest TEXT NOT NULL,
               expires_unix_ms INTEGER,
               PRIMARY KEY(owner_kind, owner_id, digest),
               FOREIGN KEY(digest) REFERENCES blobs(digest) ON DELETE CASCADE
             );
             CREATE INDEX IF NOT EXISTS blob_references_expiry
               ON blob_references(expires_unix_ms);
             CREATE TRIGGER IF NOT EXISTS blob_reference_insert
             AFTER INSERT ON blob_references BEGIN
               UPDATE blobs SET reference_count = reference_count + 1 WHERE digest = NEW.digest;
             END;
             CREATE TRIGGER IF NOT EXISTS blob_reference_delete
             AFTER DELETE ON blob_references BEGIN
               UPDATE blobs SET reference_count = MAX(0, reference_count - 1) WHERE digest = OLD.digest;
             END;",
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

    /// Report which of `digests` the store does not have.
    ///
    /// One chunked statement rather than a per-digest probe: at the
    /// `MAX_FIND_DIGESTS` ceiling the old loop ran 512 single-row queries and
    /// allocated an owned `String` per hit, all while holding the metadata mutex
    /// that every other blob operation needs. `SQLITE_MAX_VARIABLE_NUMBER`
    /// comfortably exceeds one chunk, and a digest that repeats across chunks
    /// only affects the presence set, which is what the answer depends on.
    pub fn find_missing(&self, digests: &[BlobDigest]) -> Result<Vec<BlobDigest>, BlobError> {
        if digests.len() > MAX_FIND_DIGESTS {
            return Err(BlobError::TooManyDigests);
        }
        let connection = self.inner.metadata.lock().map_err(|_| BlobError::Worker)?;
        let mut present = HashSet::with_capacity(digests.len());
        for chunk in digests.chunks(FIND_CHUNK) {
            let mut statement = connection.prepare(&format!(
                "SELECT digest FROM blobs WHERE digest IN ({})",
                std::iter::repeat_n("?", chunk.len())
                    .collect::<Vec<_>>()
                    .join(",")
            ))?;
            let params: Vec<&str> = chunk.iter().map(|digest| digest.as_str()).collect();
            present.extend(
                statement
                    .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                        row.get::<_, String>(0)
                    })?
                    .collect::<Result<HashSet<String>, _>>()?,
            );
        }
        Ok(digests
            .iter()
            .filter(|digest| !present.contains(digest.as_str()))
            .cloned()
            .collect())
    }

    pub async fn prepare_upload(
        &self,
        digest: &BlobDigest,
        declared_length: u64,
    ) -> Result<bool, BlobError> {
        if declared_length > MAX_BLOB_BYTES {
            return Err(BlobError::TooLarge);
        }
        if self.path_for(digest).exists() {
            self.verify_existing(digest).await?;
            return Ok(false);
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
        if (used.max(0) as u64).saturating_add(declared_length) > self.inner.quota_bytes {
            return Err(BlobError::QuotaExceeded);
        }
        Ok(true)
    }

    pub fn stored_size(&self, digest: &BlobDigest) -> Result<u64, BlobError> {
        let size: Option<i64> = self
            .inner
            .metadata
            .lock()
            .map_err(|_| BlobError::Worker)?
            .query_row(
                "SELECT size_bytes FROM blobs WHERE digest = ?1",
                [digest.as_str()],
                |row| row.get(0),
            )
            .optional()?;
        size.map(|size| size.max(0) as u64)
            .ok_or(BlobError::NotFound)
    }

    /// Retain content before publishing a new workspace or artifact reference.
    /// Expired references are safe to reclaim; `None` is reserved for live workspaces.
    pub fn retain(
        &self,
        owner_kind: &str,
        owner_id: &str,
        digests: &[BlobDigest],
        expires_unix_ms: Option<u64>,
    ) -> Result<(), BlobError> {
        let mut connection = self.inner.metadata.lock().map_err(|_| BlobError::Worker)?;
        let transaction = connection.transaction()?;
        for digest in digests {
            Self::retain_one(&transaction, owner_kind, owner_id, digest, expires_unix_ms)?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// Retain one digest per distinct owner, all in a single durable
    /// transaction.
    ///
    /// `retain` opens and commits its own transaction per call and the store runs
    /// `PRAGMA synchronous=FULL`, so calling it once per artifact record meant up
    /// to `MAX_ARTIFACT_FILES` (4096) fsync-ing commits for a single execution.
    /// The per-digest existence check and upsert are unchanged — they are simply
    /// evaluated in one transaction now.
    pub fn retain_per_owner(
        &self,
        owner_kind: &str,
        entries: &[(&str, BlobDigest)],
        expires_unix_ms: Option<u64>,
    ) -> Result<(), BlobError> {
        if entries.is_empty() {
            return Ok(());
        }
        let mut connection = self.inner.metadata.lock().map_err(|_| BlobError::Worker)?;
        let transaction = connection.transaction()?;
        for (owner_id, digest) in entries {
            Self::retain_one(&transaction, owner_kind, owner_id, digest, expires_unix_ms)?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// One existence check and one upsert inside an open transaction.
    fn retain_one(
        transaction: &rusqlite::Transaction<'_>,
        owner_kind: &str,
        owner_id: &str,
        digest: &BlobDigest,
        expires_unix_ms: Option<u64>,
    ) -> Result<(), BlobError> {
        let exists: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM blobs WHERE digest = ?1)",
            [digest.as_str()],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(BlobError::NotFound);
        }
        transaction.execute(
            "INSERT INTO blob_references(owner_kind, owner_id, digest, expires_unix_ms)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(owner_kind, owner_id, digest) DO UPDATE
                   SET expires_unix_ms = excluded.expires_unix_ms",
            params![
                owner_kind,
                owner_id,
                digest.as_str(),
                expires_unix_ms.map(|v| v as i64)
            ],
        )?;
        Ok(())
    }

    pub fn set_reference_expiry(
        &self,
        owner_kind: &str,
        owner_id: &str,
        expires_unix_ms: u64,
    ) -> Result<(), BlobError> {
        self.inner
            .metadata
            .lock()
            .map_err(|_| BlobError::Worker)?
            .execute(
                "UPDATE blob_references SET expires_unix_ms = ?3
                 WHERE owner_kind = ?1 AND owner_id = ?2",
                params![owner_kind, owner_id, expires_unix_ms as i64],
            )?;
        Ok(())
    }

    pub fn clear_reference_expiry(
        &self,
        owner_kind: &str,
        owner_id: &str,
    ) -> Result<(), BlobError> {
        self.inner
            .metadata
            .lock()
            .map_err(|_| BlobError::Worker)?
            .execute(
                "UPDATE blob_references SET expires_unix_ms = NULL
                 WHERE owner_kind = ?1 AND owner_id = ?2",
                params![owner_kind, owner_id],
            )?;
        Ok(())
    }

    pub fn release_references(&self, owner_kind: &str, owner_id: &str) -> Result<(), BlobError> {
        self.inner
            .metadata
            .lock()
            .map_err(|_| BlobError::Worker)?
            .execute(
                "DELETE FROM blob_references WHERE owner_kind = ?1 AND owner_id = ?2",
                params![owner_kind, owner_id],
            )?;
        Ok(())
    }

    /// List distinct blob-reference owners for one owner kind, bounded by
    /// `limit`. Used by restart reconciliation to find orphan references
    /// without scanning blob bytes.
    ///
    /// Owners are returned in stable `owner_id` order.
    ///
    /// `after` is an exclusive resume cursor: pass the last owner from the
    /// previous page to continue past it. The ordering is load-bearing, not
    /// cosmetic — without it a bounded sweep always re-reads the same arbitrary
    /// first page, so any owner positioned beyond `limit` could never be
    /// examined and its references would be pinned forever.
    pub fn reference_owners(
        &self,
        owner_kind: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>, BlobError> {
        let connection = self.inner.metadata.lock().map_err(|_| BlobError::Worker)?;
        let mut statement = connection.prepare(
            "SELECT DISTINCT owner_id FROM blob_references
             WHERE owner_kind = ?1 AND (?2 IS NULL OR owner_id > ?2)
             ORDER BY owner_id LIMIT ?3",
        )?;
        Ok(statement
            .query_map(
                params![owner_kind, after, limit.min(i64::MAX as usize) as i64],
                |row| row.get(0),
            )?
            .collect::<Result<_, _>>()?)
    }

    /// Unreferenced blobs, coldest first, bounded by `limit`.
    ///
    /// Shared by the dry run and the real run on purpose. They select at
    /// *different points* in the transaction — a preview before expired
    /// references are deleted, the real pass after — so the same statement
    /// must be evaluated in both places. Two hand-copied copies of it are what
    /// let the two paths drift apart and made `NodeGcReport.blob_candidates`
    /// under-predict what `--apply` would actually remove.
    fn unreferenced_candidates(
        transaction: &rusqlite::Transaction<'_>,
        limit: usize,
    ) -> Result<Vec<(String, u64)>, BlobError> {
        let mut statement = transaction.prepare(
            "SELECT digest, size_bytes FROM blobs
             WHERE NOT EXISTS (SELECT 1 FROM blob_references r WHERE r.digest = blobs.digest)
             ORDER BY last_used_unix_ms ASC LIMIT ?1",
        )?;
        Ok(statement
            .query_map([limit.min(i64::MAX as usize) as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?.max(0) as u64,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }

    /// Remove expired references and then a bounded number of unreferenced blobs.
    pub async fn garbage_collect(
        &self,
        now_unix_ms: u64,
        limit: usize,
        dry_run: bool,
    ) -> Result<GcReport, BlobError> {
        let _guard = self.inner.write_lock.lock().await;
        let mut connection = self.inner.metadata.lock().map_err(|_| BlobError::Worker)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let expired_references: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM blob_references WHERE expires_unix_ms IS NOT NULL AND expires_unix_ms <= ?1",
            [now_unix_ms as i64],
            |row| row.get::<_, i64>(0),
        )?;
        let mut candidates = Self::unreferenced_candidates(&transaction, limit)?;
        let mut removed_blobs = 0u64;
        let mut removed_bytes = 0u64;
        let mut expired_references_removed = 0u64;
        if !dry_run {
            expired_references_removed = transaction.execute(
                "DELETE FROM blob_references WHERE rowid IN
                 (SELECT rowid FROM blob_references WHERE expires_unix_ms IS NOT NULL
                   AND expires_unix_ms <= ?1 LIMIT ?2)",
                params![now_unix_ms as i64, limit.min(i64::MAX as usize) as i64],
            )? as u64;
            candidates = Self::unreferenced_candidates(&transaction, limit)?;
            let mut unlink_error: Option<io::Error> = None;
            for (encoded, size) in &candidates {
                let Ok(digest) = BlobDigest::parse(encoded.clone()) else {
                    continue;
                };
                let path = self.path_for(&digest);
                match fs::remove_file(&path) {
                    Ok(()) => {
                        transaction.execute(
                            "DELETE FROM blobs WHERE digest = ?1 AND NOT EXISTS
                               (SELECT 1 FROM blob_references WHERE digest = ?1)",
                            [digest.as_str()],
                        )?;
                        removed_blobs += 1;
                        removed_bytes = removed_bytes.saturating_add(*size);
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        transaction.execute(
                            "DELETE FROM blobs WHERE digest = ?1 AND NOT EXISTS
                               (SELECT 1 FROM blob_references WHERE digest = ?1)",
                            [digest.as_str()],
                        )?;
                    }
                    Err(error) => {
                        // Stop touching the filesystem, but do not abandon the
                        // transaction. Earlier candidates in this loop already had
                        // their files unlinked; rolling their `DELETE`s back would
                        // resurrect rows for blobs that no longer exist on disk,
                        // which then count against `blob_quota_bytes`, report the
                        // digest as present to `find_missing`, and surface as an
                        // opaque error on the next read. A row whose file could not
                        // be unlinked is already the consistent state, so commit
                        // what is true and report the error afterwards.
                        unlink_error.get_or_insert(error);
                        break;
                    }
                }
            }
            transaction.commit()?;
            if let Some(error) = unlink_error {
                return Err(BlobError::Io(error));
            }
        } else {
            transaction.commit()?;
        }
        Ok(GcReport {
            expired_references: expired_references.max(0) as u64,
            expired_references_removed,
            candidate_blobs: candidates.len() as u64,
            candidate_bytes: candidates
                .iter()
                .fold(0u64, |sum, (_, size)| sum.saturating_add(*size)),
            removed_blobs,
            removed_bytes,
        })
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
        // Same arithmetic as `prepare_upload` above, saturating in both places.
        // Two adjacent copies of one quota check with different overflow
        // behaviour is a latent trap even though neither is reachable today: the
        // sum only overflows near `u64::MAX`, and SQLite's `sum()` returns REAL
        // past `i64::MAX`, so the read fails closed first.
        if (used.max(0) as u64).saturating_add(declared_length) > self.inner.quota_bytes {
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

pub(super) fn unix_millis() -> i64 {
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
        assert!(matches!(
            store
                .prepare_upload(&BlobDigest::from_bytes(b"too large"), MAX_BLOB_BYTES + 1)
                .await,
            Err(BlobError::TooLarge)
        ));
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

    #[tokio::test]
    async fn find_missing_reports_across_the_chunk_boundary() {
        // `find_missing` uses a chunked `IN (...)` statement; this crosses the
        // chunk boundary, includes an absent digest, a repeat, and the empty
        // request, so a chunking bug cannot hide.
        let temp = TempDir::new().unwrap();
        let store = BlobStore::open(temp.path().join("blobs"), 1024 * 1024).unwrap();
        let mut stored = Vec::new();
        for index in 0..FIND_CHUNK + 5 {
            let payload = format!("stored-{index}").into_bytes();
            let digest = BlobDigest::from_bytes(&payload);
            store
                .put_stream(
                    digest.clone(),
                    payload.len() as u64,
                    stream::iter([Ok::<_, ()>(Bytes::from(payload))]),
                )
                .await
                .unwrap();
            stored.push(digest);
        }
        let absent = BlobDigest::from_bytes(b"absent");
        let mut queried = stored.clone();
        queried.push(absent.clone());
        queried.push(stored[0].clone());
        let missing = store.find_missing(&queried).unwrap();
        assert_eq!(missing, vec![absent]);
        assert!(store.find_missing(&[]).unwrap().is_empty());
        assert!(matches!(
            store.find_missing(&vec![stored[0].clone(); MAX_FIND_DIGESTS + 1]),
            Err(BlobError::TooManyDigests)
        ));
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn gc_commits_unlinked_rows_even_when_a_later_unlink_fails() {
        // Regression: the unlink loop returned `Err` on the first non-`NotFound`
        // failure with the transaction still open, so rusqlite rolled it back —
        // including the `DELETE`s for candidates already unlinked. That left
        // `blobs` rows pointing at files that no longer existed, which inflated
        // `SUM(size_bytes)` into a spurious `QuotaExceeded` and made
        // `find_missing` report the digest as present so a client skipped the
        // re-upload that would have repaired it.
        let temp = TempDir::new().unwrap();
        let store = BlobStore::open(temp.path().join("blobs"), 1024 * 1024).unwrap();

        let first = BlobDigest::from_bytes(b"aaaaa");
        store
            .put_stream(
                first.clone(),
                5,
                stream::iter([Ok::<_, ()>(Bytes::from_static(b"aaaaa"))]),
            )
            .await
            .unwrap();
        // `garbage_collect` orders candidates by `last_used_unix_ms ASC` at
        // millisecond resolution, so make `blocked` strictly newer.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let blocked = BlobDigest::from_bytes(b"bbbbbbb");
        store
            .put_stream(
                blocked.clone(),
                7,
                stream::iter([Ok::<_, ()>(Bytes::from_static(b"bbbbbbb"))]),
            )
            .await
            .unwrap();

        // `remove_file` against a non-empty directory fails with something other
        // than `NotFound`, which is the shape of error the pass must survive.
        let blocked_path = store.path_for(&blocked);
        fs::remove_file(&blocked_path).unwrap();
        fs::create_dir(&blocked_path).unwrap();
        fs::write(blocked_path.join("child"), b"x").unwrap();

        assert!(matches!(
            store.garbage_collect(0, 8, false).await,
            Err(BlobError::Io(_))
        ));

        // The unlink that already happened must not be undone by the rollback.
        assert!(!store.path_for(&first).exists());
        assert_eq!(
            store.find_missing(std::slice::from_ref(&first)).unwrap(),
            vec![first.clone()]
        );
        // The blob whose unlink failed keeps its row: its file is still present,
        // so the row and the filesystem still agree.
        assert!(
            store
                .find_missing(std::slice::from_ref(&blocked))
                .unwrap()
                .is_empty()
        );
        fs::remove_dir_all(&blocked_path).unwrap();
    }
}
