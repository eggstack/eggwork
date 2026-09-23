//! Declared output capture, artifact metadata, retention, and bounded cleanup.

use crate::blob::{BlobError, BlobStore};
use eggwork_core::{
    ArtifactId, ArtifactRecord, ArtifactType, BlobDigest, DeclaredOutput, ExecutionGeneration,
    ExecutionId, PrincipalId, RelativePath, WorkspaceEntry, WorkspaceManifest,
};
use futures_util::stream;
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

pub const DEFAULT_RETENTION_MILLIS: u64 = 30 * 24 * 60 * 60 * 1000;
pub const MAX_ARTIFACT_FILES: usize = 4096;
pub const MAX_ARTIFACT_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum ArtifactError {
    #[error("declared artifact path is missing")]
    MissingRequired,
    #[error("declared artifact tree contains an unsupported file type")]
    UnsupportedType,
    #[error("declared artifact tree exceeds its capture bounds")]
    Limit,
    #[error("declared artifact tree is invalid")]
    InvalidTree,
    #[error("artifact capture failed")]
    Io(#[from] io::Error),
    #[error("artifact blob operation failed")]
    Blob(#[from] BlobError),
    #[error("artifact metadata operation failed")]
    Sql(#[from] rusqlite::Error),
    #[error("artifact record encoding failed")]
    Json(#[from] serde_json::Error),
    #[error("artifact worker failed")]
    Worker,
}

#[derive(Clone)]
pub struct ArtifactStore {
    connection: Arc<Mutex<Connection>>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ArtifactGcReport {
    pub candidate_artifacts: u64,
    pub deleted_artifacts: u64,
}

type DiscoveredFiles = Vec<(RelativePath, PathBuf, bool)>;
type DiscoveredTree = (DiscoveredFiles, Vec<RelativePath>);

impl ArtifactStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ArtifactError> {
        if let Some(parent) = path.as_ref().parent() {
            fs::create_dir_all(parent)?;
        }
        if !path.as_ref().exists() {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let _file = options.open(path.as_ref())?;
        }
        let connection = Connection::open(path)?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             CREATE TABLE IF NOT EXISTS artifacts (
               artifact_id TEXT PRIMARY KEY,
               execution_id TEXT NOT NULL,
               generation INTEGER NOT NULL,
               principal_id TEXT NOT NULL,
               record_json BLOB NOT NULL,
               created_unix_ms INTEGER NOT NULL,
               expires_unix_ms INTEGER NOT NULL,
               UNIQUE(execution_id, generation, principal_id, record_json)
             );
             CREATE INDEX IF NOT EXISTS artifacts_owner
               ON artifacts(execution_id, generation, principal_id, created_unix_ms);",
        )?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    pub fn insert_many(
        &self,
        records: &[ArtifactRecord],
        principal: &PrincipalId,
    ) -> Result<(), ArtifactError> {
        let mut connection = self.connection.lock().map_err(|_| ArtifactError::Worker)?;
        let transaction = connection.transaction()?;
        for record in records {
            transaction.execute(
                "INSERT INTO artifacts(artifact_id, execution_id, generation, principal_id,
                   record_json, created_unix_ms, expires_unix_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    record.artifact_id.as_str(),
                    record.execution_id.as_str(),
                    record.generation.get() as i64,
                    principal.as_str(),
                    serde_json::to_vec(record)?,
                    record.created_unix_ms as i64,
                    record.expires_unix_ms as i64,
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn list(
        &self,
        execution_id: &ExecutionId,
        generation: ExecutionGeneration,
        principal: &PrincipalId,
    ) -> Result<Vec<ArtifactRecord>, ArtifactError> {
        let connection = self.connection.lock().map_err(|_| ArtifactError::Worker)?;
        let mut statement = connection.prepare(
            "SELECT record_json FROM artifacts WHERE execution_id = ?1
             AND generation = ?2 AND principal_id = ?3 AND expires_unix_ms > ?4
             ORDER BY created_unix_ms, artifact_id",
        )?;
        let rows = statement.query_map(
            params![
                execution_id.as_str(),
                generation.get() as i64,
                principal.as_str(),
                now_unix_ms() as i64
            ],
            |row| row.get::<_, Vec<u8>>(0),
        )?;
        rows.map(|row| {
            let bytes = row?;
            serde_json::from_slice(&bytes).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Blob,
                    Box::new(error),
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(ArtifactError::from)
    }

    pub fn get(
        &self,
        artifact_id: &ArtifactId,
        principal: &PrincipalId,
    ) -> Result<ArtifactRecord, ArtifactError> {
        let encoded: Option<Vec<u8>> = self
            .connection
            .lock()
            .map_err(|_| ArtifactError::Worker)?
            .query_row(
                "SELECT record_json FROM artifacts WHERE artifact_id = ?1 AND principal_id = ?2
                 AND expires_unix_ms > ?3",
                params![
                    artifact_id.as_str(),
                    principal.as_str(),
                    now_unix_ms() as i64
                ],
                |row| row.get(0),
            )
            .optional()?;
        encoded
            .map(|bytes| serde_json::from_slice(&bytes).map_err(ArtifactError::from))
            .unwrap_or_else(|| Err(ArtifactError::MissingRequired))
    }

    pub fn garbage_collect(
        &self,
        now_unix_ms: u64,
        limit: usize,
        dry_run: bool,
    ) -> Result<ArtifactGcReport, ArtifactError> {
        let connection = self.connection.lock().map_err(|_| ArtifactError::Worker)?;
        let count: i64 = connection.query_row(
            "SELECT COUNT(*) FROM artifacts WHERE expires_unix_ms <= ?1",
            [now_unix_ms as i64],
            |row| row.get(0),
        )?;
        let limit = limit.min(i64::MAX as usize) as i64;
        let candidate_count: i64 = connection.query_row(
            "SELECT COUNT(*) FROM (SELECT artifact_id FROM artifacts
             WHERE expires_unix_ms <= ?1 ORDER BY expires_unix_ms LIMIT ?2)",
            params![now_unix_ms as i64, limit],
            |row| row.get(0),
        )?;
        if !dry_run {
            connection.execute(
                "DELETE FROM artifacts WHERE artifact_id IN
                 (SELECT artifact_id FROM artifacts WHERE expires_unix_ms <= ?1
                  ORDER BY expires_unix_ms LIMIT ?2)",
                params![now_unix_ms as i64, limit],
            )?;
        }
        Ok(ArtifactGcReport {
            candidate_artifacts: candidate_count.max(0).min(count.max(0)) as u64,
            deleted_artifacts: if dry_run {
                0
            } else {
                candidate_count.max(0).min(count.max(0)) as u64
            },
        })
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn capture_declared(
    store: &ArtifactStore,
    blobs: &BlobStore,
    root: &Path,
    outputs: &[DeclaredOutput],
    execution_id: &ExecutionId,
    generation: ExecutionGeneration,
    principal: &PrincipalId,
    expires_unix_ms: u64,
) -> Result<u32, ArtifactError> {
    let root = root.to_path_buf();
    let outputs = outputs.to_vec();
    let discovery_root = root.clone();
    let (files, directories) =
        tokio::task::spawn_blocking(move || discover_outputs(&discovery_root, &outputs))
            .await
            .map_err(|_| ArtifactError::Worker)??;

    let discovered_count = files.len();
    let empty_digest = BlobDigest::from_bytes(b"");
    let preflight = WorkspaceManifest {
        schema_version: 1,
        entries: files
            .iter()
            .map(|(path, _, executable)| WorkspaceEntry::File {
                path: path.clone(),
                digest: empty_digest.clone(),
                size_bytes: 0,
                executable: *executable,
            })
            .chain(
                directories
                    .iter()
                    .cloned()
                    .map(|path| WorkspaceEntry::Directory { path }),
            )
            .collect(),
    };
    preflight
        .validate()
        .map_err(|_| ArtifactError::InvalidTree)?;
    let mut entries = Vec::with_capacity(discovered_count + directories.len());
    let mut records = Vec::with_capacity(discovered_count);
    let mut total = 0u64;
    for (path, _file_path, _discovered_executable) in files {
        let (digest, size_bytes, executable) = capture_blob(
            blobs,
            &root,
            &path,
            MAX_ARTIFACT_TOTAL_BYTES.saturating_sub(total),
        )
        .await?;
        total = total.saturating_add(size_bytes);
        let record = ArtifactRecord {
            artifact_id: ArtifactId::new(uuid::Uuid::new_v4().to_string())
                .map_err(|_| ArtifactError::InvalidTree)?,
            execution_id: execution_id.clone(),
            generation,
            path: path.clone(),
            kind: ArtifactType::File,
            digest: digest.clone(),
            size_bytes,
            executable,
            created_unix_ms: now_unix_ms(),
            expires_unix_ms,
        };
        records.push(record.clone());
        entries.push(WorkspaceEntry::File {
            path,
            digest: record.digest,
            size_bytes,
            executable,
        });
    }
    entries.extend(
        directories
            .into_iter()
            .map(|path| WorkspaceEntry::Directory { path }),
    );
    let manifest = WorkspaceManifest {
        schema_version: 1,
        entries,
    };
    manifest
        .validate()
        .map_err(|_| ArtifactError::InvalidTree)?;
    for record in &records {
        // Establish the blob reference before metadata becomes visible. A crash
        // here can retain extra bytes until expiry, never lose live data.
        blobs.retain(
            "artifact",
            record.artifact_id.as_str(),
            std::slice::from_ref(&record.digest),
            Some(expires_unix_ms),
        )?;
    }
    store.insert_many(&records, principal)?;
    Ok(discovered_count as u32)
}

async fn capture_blob(
    blobs: &BlobStore,
    root: &Path,
    relative: &RelativePath,
    remaining_bytes: u64,
) -> Result<(BlobDigest, u64, bool), ArtifactError> {
    let file = open_file_beneath(root, relative.as_str())?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(ArtifactError::UnsupportedType);
    }
    #[cfg(unix)]
    let executable = {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    };
    #[cfg(not(unix))]
    let executable = false;
    let mut file = tokio::fs::File::from_std(file);
    let mut hash = Sha256::new();
    let mut size = 0u64;
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        size = size.saturating_add(read as u64);
        if size > crate::blob::MAX_BLOB_BYTES || size > remaining_bytes {
            return Err(ArtifactError::Limit);
        }
        hash.update(&buffer[..read]);
    }
    let digest =
        BlobDigest::parse(hex::encode(hash.finalize())).map_err(|_| ArtifactError::InvalidTree)?;
    file.seek(SeekFrom::Start(0)).await?;
    let input = stream::unfold(file, |mut file| async move {
        let mut buffer = vec![0u8; 64 * 1024];
        match file.read(&mut buffer).await {
            Ok(0) => None,
            Ok(read) => {
                buffer.truncate(read);
                Some((Ok::<_, BlobError>(bytes::Bytes::from(buffer)), file))
            }
            Err(_) => Some((
                Err(BlobError::Io(io::Error::other("artifact read failed"))),
                file,
            )),
        }
    });
    blobs.put_stream(digest.clone(), size, input).await?;
    Ok((digest, size, executable))
}

#[cfg(unix)]
fn open_file_beneath(root: &Path, relative: &str) -> io::Result<fs::File> {
    use rustix::fs::{Mode, OFlags, open, openat};
    use std::os::fd::OwnedFd;

    let root_fd = open(
        root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )?;
    let components = relative.split('/').collect::<Vec<_>>();
    let (filename, parents) = components
        .split_last()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty artifact path"))?;
    let mut current: OwnedFd = root_fd;
    for parent in parents {
        current = openat(
            &current,
            *parent,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )?;
    }
    let file = openat(
        &current,
        *filename,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )?;
    Ok(fs::File::from(file))
}

#[cfg(not(unix))]
fn open_file_beneath(_root: &Path, _relative: &str) -> io::Result<fs::File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "safe artifact capture is unavailable on this platform",
    ))
}

fn discover_outputs(
    root: &Path,
    outputs: &[DeclaredOutput],
) -> Result<DiscoveredTree, ArtifactError> {
    let root_metadata = fs::symlink_metadata(root)?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(ArtifactError::UnsupportedType);
    }
    let mut files = BTreeMap::<String, (PathBuf, bool)>::new();
    let mut directories = BTreeSet::<String>::new();
    for output in outputs {
        let path = root.join(output.path.as_str());
        let metadata = match safe_metadata_under_root(root, output.path.as_str()) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound && !output.required => continue,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(ArtifactError::MissingRequired);
            }
            Err(error) if error.kind() == io::ErrorKind::InvalidInput => {
                return Err(ArtifactError::UnsupportedType);
            }
            Err(error) => return Err(error.into()),
        };
        visit_output(
            root,
            &path,
            output.path.as_str(),
            metadata,
            &mut files,
            &mut directories,
        )?;
    }
    if files.len() > MAX_ARTIFACT_FILES {
        return Err(ArtifactError::Limit);
    }
    let files = files
        .into_iter()
        .map(|(name, (path, executable))| {
            Ok((
                RelativePath::new(name).map_err(|_| ArtifactError::InvalidTree)?,
                path,
                executable,
            ))
        })
        .collect::<Result<Vec<_>, ArtifactError>>()?;
    let directories = directories
        .into_iter()
        .map(|name| RelativePath::new(name).map_err(|_| ArtifactError::InvalidTree))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((files, directories))
}

fn safe_metadata_under_root(root: &Path, relative: &str) -> io::Result<fs::Metadata> {
    let mut current = root.to_path_buf();
    let components = relative.split('/').collect::<Vec<_>>();
    let mut metadata = None;
    for (index, component) in components.iter().enumerate() {
        current.push(component);
        let current_metadata = fs::symlink_metadata(&current)?;
        if current_metadata.file_type().is_symlink()
            || (index + 1 < components.len() && !current_metadata.is_dir())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "artifact path contains a link or non-directory parent",
            ));
        }
        metadata = Some(current_metadata);
    }
    metadata.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty artifact path"))
}

fn visit_output(
    root: &Path,
    path: &Path,
    logical: &str,
    metadata: fs::Metadata,
    files: &mut BTreeMap<String, (PathBuf, bool)>,
    directories: &mut BTreeSet<String>,
) -> Result<(), ArtifactError> {
    if metadata.file_type().is_symlink() {
        return Err(ArtifactError::UnsupportedType);
    }
    if metadata.is_file() {
        let logical =
            RelativePath::new(logical.to_owned()).map_err(|_| ArtifactError::InvalidTree)?;
        add_parent_directories(logical.as_str(), directories)?;
        files.entry(logical.as_str().to_owned()).or_insert_with(|| {
            #[cfg(unix)]
            let executable = {
                use std::os::unix::fs::PermissionsExt;
                metadata.permissions().mode() & 0o111 != 0
            };
            #[cfg(not(unix))]
            let executable = false;
            (path.to_path_buf(), executable)
        });
    } else if metadata.is_dir() {
        let logical_path =
            RelativePath::new(logical.to_owned()).map_err(|_| ArtifactError::InvalidTree)?;
        directories.insert(logical_path.as_str().to_owned());
        add_parent_directories(logical_path.as_str(), directories)?;
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let child_path = entry.path();
            let child_metadata = fs::symlink_metadata(&child_path)?;
            let relative = child_path
                .strip_prefix(root)
                .map_err(|_| ArtifactError::InvalidTree)?
                .to_str()
                .ok_or(ArtifactError::InvalidTree)?
                .replace(std::path::MAIN_SEPARATOR, "/");
            visit_output(
                root,
                &child_path,
                &relative,
                child_metadata,
                files,
                directories,
            )?;
            if files.len().saturating_add(directories.len()) > MAX_ARTIFACT_FILES {
                return Err(ArtifactError::Limit);
            }
        }
    } else {
        return Err(ArtifactError::UnsupportedType);
    }
    Ok(())
}

fn add_parent_directories(
    path: &str,
    directories: &mut BTreeSet<String>,
) -> Result<(), ArtifactError> {
    let mut end = 0;
    while let Some(offset) = path[end..].find('/') {
        end += offset;
        let parent = &path[..end];
        RelativePath::new(parent.to_owned()).map_err(|_| ArtifactError::InvalidTree)?;
        directories.insert(parent.to_owned());
        end += 1;
    }
    Ok(())
}

pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;

    #[tokio::test]
    async fn captures_only_declared_files_and_streams_verified_content() {
        let temp = tempfile::tempdir().unwrap();
        let blobs = BlobStore::open(temp.path().join("blobs"), 1024 * 1024).unwrap();
        let artifacts = ArtifactStore::open(temp.path().join("artifacts.sqlite")).unwrap();
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(workspace.join("out/nested")).unwrap();
        fs::write(workspace.join("out/nested/result.bin"), b"finished output").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                workspace.join("out/nested/result.bin"),
                fs::Permissions::from_mode(0o700),
            )
            .unwrap();
        }
        fs::write(workspace.join("secret.txt"), b"undeclared").unwrap();
        let outputs = vec![DeclaredOutput {
            path: RelativePath::new("out".to_owned()).unwrap(),
            required: true,
        }];
        let execution = ExecutionId::new("artifact-execution").unwrap();
        let generation = ExecutionGeneration::new(1).unwrap();
        let principal = PrincipalId::new("controller-a").unwrap();
        let expiry = now_unix_ms() + 60_000;

        let count = capture_declared(
            &artifacts, &blobs, &workspace, &outputs, &execution, generation, &principal, expiry,
        )
        .await
        .unwrap();

        assert_eq!(count, 1);
        let records = artifacts.list(&execution, generation, &principal).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].path.as_str(), "out/nested/result.bin");
        assert_eq!(records[0].kind, ArtifactType::File);
        #[cfg(unix)]
        assert!(records[0].executable);
        assert_eq!(records[0].size_bytes, 15);
        assert_eq!(
            records[0].digest,
            BlobDigest::from_bytes(b"finished output")
        );
        assert!(
            artifacts
                .get(
                    &records[0].artifact_id,
                    &PrincipalId::new("controller-b").unwrap()
                )
                .is_err()
        );

        let mut downloaded = Vec::new();
        let mut blob = blobs.open_verified(&records[0].digest).await.unwrap();
        tokio::io::AsyncReadExt::read_to_end(&mut blob, &mut downloaded)
            .await
            .unwrap();
        assert_eq!(downloaded, b"finished output");
        assert!(
            !records
                .iter()
                .any(|record| record.path.as_str() == "secret.txt")
        );
        let dry_gc = artifacts.garbage_collect(expiry, 1, true).unwrap();
        assert_eq!(dry_gc.candidate_artifacts, 1);
        assert_eq!(
            artifacts
                .list(&execution, generation, &principal)
                .unwrap()
                .len(),
            1
        );
        let deleted = artifacts.garbage_collect(expiry, 1, false).unwrap();
        assert_eq!(deleted.deleted_artifacts, 1);
        assert!(
            artifacts
                .list(&execution, generation, &principal)
                .unwrap()
                .is_empty()
        );
        let blob_gc = blobs.garbage_collect(expiry, 1, false).await.unwrap();
        assert_eq!(blob_gc.removed_blobs, 1);
    }

    #[tokio::test]
    async fn missing_required_and_symlink_outputs_fail_without_records() {
        let temp = tempfile::tempdir().unwrap();
        let blobs = BlobStore::open(temp.path().join("blobs"), 1024 * 1024).unwrap();
        let artifacts = ArtifactStore::open(temp.path().join("artifacts.sqlite")).unwrap();
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let execution = ExecutionId::new("artifact-missing").unwrap();
        let generation = ExecutionGeneration::new(1).unwrap();
        let principal = PrincipalId::new("controller-a").unwrap();
        let missing = vec![DeclaredOutput {
            path: RelativePath::new("required.bin".to_owned()).unwrap(),
            required: true,
        }];
        assert!(
            capture_declared(
                &artifacts,
                &blobs,
                &workspace,
                &missing,
                &execution,
                generation,
                &principal,
                now_unix_ms() + 60_000,
            )
            .await
            .is_err()
        );
        assert!(
            artifacts
                .list(&execution, generation, &principal)
                .unwrap()
                .is_empty()
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let outside = temp.path().join("outside");
            fs::write(&outside, b"must not capture").unwrap();
            symlink(&outside, workspace.join("linked")).unwrap();
            let symlink_output = vec![DeclaredOutput {
                path: RelativePath::new("linked".to_owned()).unwrap(),
                required: true,
            }];
            assert!(matches!(
                capture_declared(
                    &artifacts,
                    &blobs,
                    &workspace,
                    &symlink_output,
                    &execution,
                    generation,
                    &principal,
                    now_unix_ms() + 60_000,
                )
                .await,
                Err(ArtifactError::UnsupportedType)
            ));
            let outside_dir = temp.path().join("outside-dir");
            fs::create_dir_all(&outside_dir).unwrap();
            fs::write(outside_dir.join("secret.txt"), b"must not escape").unwrap();
            symlink(&outside_dir, workspace.join("linked-dir")).unwrap();
            let nested_symlink_output = vec![DeclaredOutput {
                path: RelativePath::new("linked-dir/secret.txt".to_owned()).unwrap(),
                required: true,
            }];
            assert!(matches!(
                capture_declared(
                    &artifacts,
                    &blobs,
                    &workspace,
                    &nested_symlink_output,
                    &execution,
                    generation,
                    &principal,
                    now_unix_ms() + 60_000,
                )
                .await,
                Err(ArtifactError::UnsupportedType)
            ));
            assert!(
                artifacts
                    .list(&execution, generation, &principal)
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[tokio::test]
    async fn disappearing_output_and_storage_quota_fail_without_publishing_records() {
        let temp = tempfile::tempdir().unwrap();
        let blobs = BlobStore::open(temp.path().join("blobs"), 4).unwrap();
        let artifacts = ArtifactStore::open(temp.path().join("artifacts.sqlite")).unwrap();
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let output_path = workspace.join("result.bin");
        fs::write(&output_path, b"12345").unwrap();
        let outputs = vec![DeclaredOutput {
            path: RelativePath::new("result.bin".to_owned()).unwrap(),
            required: true,
        }];
        let execution = ExecutionId::new("artifact-failure").unwrap();
        let generation = ExecutionGeneration::new(1).unwrap();
        let principal = PrincipalId::new("controller-a").unwrap();
        let (discovered, _) = discover_outputs(&workspace, &outputs).unwrap();
        fs::remove_file(&output_path).unwrap();
        assert!(matches!(
            capture_blob(
                &blobs,
                &workspace,
                &discovered[0].0,
                MAX_ARTIFACT_TOTAL_BYTES,
            )
            .await,
            Err(ArtifactError::Io(_))
        ));
        fs::write(&output_path, b"12345").unwrap();
        assert!(matches!(
            capture_declared(
                &artifacts,
                &blobs,
                &workspace,
                &outputs,
                &execution,
                generation,
                &principal,
                now_unix_ms() + 60_000,
            )
            .await,
            Err(ArtifactError::Blob(BlobError::QuotaExceeded))
        ));
        assert!(
            artifacts
                .list(&execution, generation, &principal)
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn retention_refs_protect_shared_blobs_and_gc_is_bounded_and_restart_safe() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("blobs");
        let blobs = BlobStore::open(&root, 1024 * 1024).unwrap();
        let bytes = bytes::Bytes::from_static(b"shared");
        let digest = BlobDigest::from_bytes(&bytes);
        blobs
            .put_stream(
                digest.clone(),
                bytes.len() as u64,
                stream::iter([Ok::<_, BlobError>(bytes)]),
            )
            .await
            .unwrap();
        blobs
            .retain("artifact", "one", std::slice::from_ref(&digest), Some(100))
            .unwrap();
        blobs
            .retain(
                "artifact",
                "two",
                std::slice::from_ref(&digest),
                Some(1_000),
            )
            .unwrap();
        blobs
            .retain(
                "reader",
                "active-reader",
                std::slice::from_ref(&digest),
                None,
            )
            .unwrap();

        let dry = blobs.garbage_collect(200, 1, true).await.unwrap();
        assert_eq!(dry.candidate_blobs, 0);
        let _ = blobs.garbage_collect(200, 1, false).await.unwrap();
        drop(blobs);
        let reopened = BlobStore::open(&root, 1024 * 1024).unwrap();
        reopened.verify_existing(&digest).await.unwrap();
        reopened.release_references("artifact", "two").unwrap();
        let live_reader_gc = reopened.garbage_collect(1_001, 1, false).await.unwrap();
        assert_eq!(live_reader_gc.removed_blobs, 0);
        reopened
            .release_references("reader", "active-reader")
            .unwrap();
        let report = reopened.garbage_collect(1_001, 1, false).await.unwrap();
        assert_eq!(report.removed_blobs, 1);
        assert!(reopened.open_verified(&digest).await.is_err());
    }

    #[tokio::test]
    async fn startup_reconciles_a_crash_after_blob_unlink_during_gc() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("blobs");
        let blobs = BlobStore::open(&root, 1024 * 1024).unwrap();
        let bytes = bytes::Bytes::from_static(b"gc cut point");
        let digest = BlobDigest::from_bytes(&bytes);
        blobs
            .put_stream(
                digest.clone(),
                bytes.len() as u64,
                stream::iter([Ok::<_, BlobError>(bytes)]),
            )
            .await
            .unwrap();
        fs::remove_file(blobs.path_for(&digest)).unwrap();
        drop(blobs);

        let reopened = BlobStore::open(&root, 1024 * 1024).unwrap();
        assert!(reopened.stored_size(&digest).is_err());
        let report = reopened
            .garbage_collect(now_unix_ms(), 1, false)
            .await
            .unwrap();
        assert_eq!(report.removed_blobs, 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn descriptor_relative_capture_rejects_parent_swapped_to_symlink() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace");
        fs::create_dir_all(root.join("tree")).unwrap();
        fs::write(root.join("tree/result.bin"), b"inside").unwrap();
        let output = DeclaredOutput {
            path: RelativePath::new("tree/result.bin".to_owned()).unwrap(),
            required: true,
        };
        let (discovered, _) = discover_outputs(&root, std::slice::from_ref(&output)).unwrap();
        let outside = temp.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("result.bin"), b"outside").unwrap();
        fs::remove_dir_all(root.join("tree")).unwrap();
        symlink(&outside, root.join("tree")).unwrap();

        let blobs = BlobStore::open(temp.path().join("blobs"), 1024).unwrap();
        assert!(matches!(
            capture_blob(&blobs, &root, &discovered[0].0, 1024).await,
            Err(ArtifactError::Io(_))
        ));
    }
}
