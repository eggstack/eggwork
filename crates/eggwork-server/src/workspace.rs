//! Validated, execution-owned workspace materialization.

use crate::blob::{BlobStore, MAX_BLOB_BYTES};
use eggwork_core::{
    ExecutionGeneration, ExecutionHandle, ExecutionId, PrincipalId, WorkspaceEntry, WorkspaceId,
    WorkspaceManifest,
};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("workspace manifest is invalid")]
    InvalidManifest,
    #[error("workspace references a missing or corrupt blob")]
    InvalidBlob,
    #[error("workspace blob size does not match its manifest")]
    BlobSizeMismatch,
    #[error("workspace storage quota exceeded")]
    QuotaExceeded,
    #[error("workspace identity conflicts with an existing workspace")]
    Conflict,
    #[error("workspace does not exist or is not ready")]
    NotFound,
    #[error("workspace metadata is unavailable")]
    Metadata(#[from] rusqlite::Error),
    #[error("workspace filesystem operation failed")]
    Io(#[from] io::Error),
    #[error("workspace manifest serialization failed")]
    Json(#[from] serde_json::Error),
    #[error("workspace manager worker failed")]
    Worker,
}

#[derive(Clone)]
pub struct WorkspaceManager {
    inner: Arc<WorkspaceManagerInner>,
}

struct WorkspaceManagerInner {
    root: PathBuf,
    quota_bytes: u64,
    metadata: Mutex<Connection>,
    materialize_lock: tokio::sync::Mutex<()>,
}

#[derive(Debug)]
pub struct ReadyWorkspace {
    pub root: PathBuf,
    pub logical_bytes: u64,
    pub manifest_digest: String,
}

impl WorkspaceManager {
    pub fn open(root: impl AsRef<Path>, quota_bytes: u64) -> Result<Self, WorkspaceError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        }
        let metadata = Connection::open(root.join("metadata.sqlite"))?;
        metadata.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             CREATE TABLE IF NOT EXISTS workspaces (
               workspace_id TEXT PRIMARY KEY,
               execution_id TEXT NOT NULL,
               generation INTEGER NOT NULL,
               principal_id TEXT NOT NULL,
               manifest_digest TEXT NOT NULL,
               storage_key TEXT NOT NULL UNIQUE,
               logical_bytes INTEGER NOT NULL,
               state TEXT NOT NULL,
               created_unix_ms INTEGER NOT NULL
             );",
        )?;
        let manager = Self {
            inner: Arc::new(WorkspaceManagerInner {
                root,
                quota_bytes,
                metadata: Mutex::new(metadata),
                materialize_lock: tokio::sync::Mutex::new(()),
            }),
        };
        manager.reconcile()?;
        Ok(manager)
    }

    pub async fn materialize(
        &self,
        workspace_id: WorkspaceId,
        handle: &ExecutionHandle,
        principal_id: &PrincipalId,
        manifest: WorkspaceManifest,
        blobs: &BlobStore,
    ) -> Result<ReadyWorkspace, WorkspaceError> {
        manifest
            .validate()
            .map_err(|_| WorkspaceError::InvalidManifest)?;
        let logical_bytes = manifest.logical_bytes();
        if logical_bytes > self.inner.quota_bytes {
            return Err(WorkspaceError::QuotaExceeded);
        }
        let manifest_digest = manifest.digest()?.as_str().to_owned();
        let _guard = self.inner.materialize_lock.lock().await;
        if let Some(existing) = self.lookup(&workspace_id)? {
            if existing.execution_id != handle.execution_id.as_str()
                || existing.generation != handle.generation.get()
                || existing.principal_id != principal_id.as_str()
                || existing.manifest_digest != manifest_digest
            {
                return Err(WorkspaceError::Conflict);
            }
            let root = self.inner.root.join(&existing.storage_key);
            if !valid_storage_key(&existing.storage_key) || !root.is_dir() {
                return Err(WorkspaceError::NotFound);
            }
            return Ok(ReadyWorkspace {
                root,
                logical_bytes: existing.logical_bytes,
                manifest_digest,
            });
        }
        let used: i64 = self
            .inner
            .metadata
            .lock()
            .map_err(|_| WorkspaceError::Worker)?
            .query_row(
                "SELECT COALESCE(SUM(logical_bytes), 0) FROM workspaces WHERE state = 'Ready'",
                [],
                |row| row.get(0),
            )?;
        if (used.max(0) as u64).saturating_add(logical_bytes) > self.inner.quota_bytes {
            return Err(WorkspaceError::QuotaExceeded);
        }

        // Validate every referenced object before creating any visible workspace state.
        for entry in &manifest.entries {
            if let WorkspaceEntry::File {
                digest, size_bytes, ..
            } = entry
            {
                if *size_bytes > MAX_BLOB_BYTES {
                    return Err(WorkspaceError::BlobSizeMismatch);
                }
                match blobs.stored_size(digest) {
                    Ok(size) if size == *size_bytes => {}
                    Ok(_) => return Err(WorkspaceError::BlobSizeMismatch),
                    Err(_) => return Err(WorkspaceError::InvalidBlob),
                }
                blobs
                    .verify_existing(digest)
                    .await
                    .map_err(|_| WorkspaceError::InvalidBlob)?;
            }
        }

        let nonce = uuid::Uuid::new_v4().to_string();
        let stage_key = format!(".staging-{nonce}");
        let storage_key = format!("workspace-{nonce}");
        let stage = self.inner.root.join(&stage_key);
        let target = self.inner.root.join(&storage_key);
        let tree_manifest = manifest.clone();
        let tree_blobs = blobs.clone();
        let tree_stage = stage.clone();
        let result = match tokio::task::spawn_blocking(move || {
            materialize_tree(&tree_stage, &tree_manifest, &tree_blobs)
        })
        .await
        {
            Ok(result) => result,
            Err(_) => {
                let _ = fs::remove_dir_all(&stage);
                return Err(WorkspaceError::Worker);
            }
        };
        if let Err(error) = result {
            let _ = fs::remove_dir_all(&stage);
            return Err(error);
        }
        if let Err(error) = fs::rename(&stage, &target) {
            let _ = fs::remove_dir_all(&stage);
            return Err(error.into());
        }
        if let Err(error) = sync_directory(&self.inner.root) {
            let _ = fs::remove_dir_all(&target);
            return Err(error.into());
        }
        let inserted = match self.inner.metadata.lock() {
            Ok(connection) => connection.execute(
                "INSERT INTO workspaces(workspace_id, execution_id, generation, principal_id,
                   manifest_digest, storage_key, logical_bytes, state, created_unix_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'Ready', ?8)",
                params![
                    workspace_id.as_str(),
                    handle.execution_id.as_str(),
                    handle.generation.get() as i64,
                    principal_id.as_str(),
                    manifest_digest,
                    storage_key,
                    logical_bytes as i64,
                    crate::blob::unix_millis(),
                ],
            ),
            Err(_) => {
                let _ = fs::remove_dir_all(&target);
                return Err(WorkspaceError::Worker);
            }
        };
        if let Err(error) = inserted {
            let _ = fs::remove_dir_all(&target);
            return Err(WorkspaceError::Metadata(error));
        }
        Ok(ReadyWorkspace {
            root: target,
            logical_bytes,
            manifest_digest,
        })
    }

    pub fn resolve(
        &self,
        workspace_id: &WorkspaceId,
        execution_id: &ExecutionId,
        generation: ExecutionGeneration,
        principal_id: &PrincipalId,
    ) -> Result<ReadyWorkspace, WorkspaceError> {
        let Some(record) = self.lookup(workspace_id)? else {
            return Err(WorkspaceError::NotFound);
        };
        if record.state != "Ready"
            || record.execution_id != execution_id.as_str()
            || record.generation != generation.get()
            || record.principal_id != principal_id.as_str()
        {
            return Err(WorkspaceError::NotFound);
        }
        if !valid_storage_key(&record.storage_key) {
            return Err(WorkspaceError::NotFound);
        }
        let root = self.inner.root.join(record.storage_key);
        if !root.is_dir() {
            return Err(WorkspaceError::NotFound);
        }
        Ok(ReadyWorkspace {
            root,
            logical_bytes: record.logical_bytes,
            manifest_digest: record.manifest_digest,
        })
    }

    fn lookup(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Option<WorkspaceRecord>, WorkspaceError> {
        let connection = self
            .inner
            .metadata
            .lock()
            .map_err(|_| WorkspaceError::Worker)?;
        connection
            .query_row(
                "SELECT execution_id, generation, principal_id, manifest_digest, storage_key,
                   logical_bytes, state FROM workspaces WHERE workspace_id = ?1",
                [workspace_id.as_str()],
                |row| {
                    Ok(WorkspaceRecord {
                        execution_id: row.get(0)?,
                        generation: row.get::<_, i64>(1)? as u64,
                        principal_id: row.get(2)?,
                        manifest_digest: row.get(3)?,
                        storage_key: row.get(4)?,
                        logical_bytes: row.get::<_, i64>(5)?.max(0) as u64,
                        state: row.get(6)?,
                    })
                },
            )
            .optional()
            .map_err(WorkspaceError::from)
    }

    fn reconcile(&self) -> Result<(), WorkspaceError> {
        let mut known: HashSet<String> = {
            let connection = self
                .inner
                .metadata
                .lock()
                .map_err(|_| WorkspaceError::Worker)?;
            let mut statement = connection.prepare("SELECT storage_key FROM workspaces")?;
            statement
                .query_map([], |row| row.get(0))?
                .collect::<Result<_, _>>()?
        };
        for key in known.clone() {
            if !valid_storage_key(&key) {
                self.inner
                    .metadata
                    .lock()
                    .map_err(|_| WorkspaceError::Worker)?
                    .execute("DELETE FROM workspaces WHERE storage_key = ?1", [&key])?;
                known.remove(&key);
            }
        }
        for entry in fs::read_dir(&self.inner.root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(".staging-") || !known.contains(&name) {
                fs::remove_dir_all(entry.path())?;
            }
        }
        for key in &known {
            if !self.inner.root.join(key).is_dir() {
                self.inner
                    .metadata
                    .lock()
                    .map_err(|_| WorkspaceError::Worker)?
                    .execute("DELETE FROM workspaces WHERE storage_key = ?1", [key])?;
            }
        }
        Ok(())
    }
}

struct WorkspaceRecord {
    execution_id: String,
    generation: u64,
    principal_id: String,
    manifest_digest: String,
    storage_key: String,
    logical_bytes: u64,
    state: String,
}

fn valid_storage_key(key: &str) -> bool {
    key.strip_prefix("workspace-")
        .and_then(|value| uuid::Uuid::parse_str(value).ok())
        .is_some()
}

fn materialize_tree(
    stage: &Path,
    manifest: &WorkspaceManifest,
    blobs: &BlobStore,
) -> Result<(), WorkspaceError> {
    fs::create_dir(stage)?;
    set_directory_permissions(stage)?;
    let mut entries = manifest.entries.iter().collect::<Vec<_>>();
    entries.sort_by(|left, right| left.path().as_str().cmp(right.path().as_str()));
    for entry in entries {
        let path = stage.join(entry.path().as_str());
        match entry {
            WorkspaceEntry::Directory { .. } => {
                fs::create_dir(&path)?;
                set_directory_permissions(&path)?;
            }
            WorkspaceEntry::File {
                digest,
                size_bytes,
                executable,
                ..
            } => {
                let source = blobs.path_for(digest);
                let mut source_file = File::open(source)?;
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(if *executable { 0o700 } else { 0o600 });
                }
                let mut destination = options.open(&path)?;
                let copied = io::copy(&mut source_file, &mut destination)?;
                if copied != *size_bytes {
                    return Err(WorkspaceError::BlobSizeMismatch);
                }
                destination.sync_all()?;
                set_file_permissions(&path, *executable)?;
            }
            WorkspaceEntry::Symlink { .. } => return Err(WorkspaceError::InvalidManifest),
        }
    }
    let mut directories = vec![stage.to_path_buf()];
    directories.extend(manifest.entries.iter().filter_map(|entry| match entry {
        WorkspaceEntry::Directory { path } => Some(stage.join(path.as_str())),
        _ => None,
    }));
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for directory in directories {
        sync_directory(&directory)?;
    }
    Ok(())
}

#[cfg(unix)]
fn set_directory_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_directory_permissions(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_file_permissions(path: &Path, executable: bool) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(
        path,
        fs::Permissions::from_mode(if executable { 0o700 } else { 0o600 }),
    )
}

#[cfg(not(unix))]
fn set_file_permissions(_path: &Path, _executable: bool) -> io::Result<()> {
    Ok(())
}

fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{future::join_all, stream};
    use tempfile::TempDir;

    fn owner_handle(id: &str) -> ExecutionHandle {
        ExecutionHandle {
            execution_id: ExecutionId::new(id).unwrap(),
            generation: ExecutionGeneration::new(1).unwrap(),
            lease_id: eggwork_core::LeaseId::new(uuid::Uuid::new_v4().to_string()).unwrap(),
        }
    }

    async fn upload_blob(blobs: &BlobStore, bytes: &'static [u8]) -> eggwork_core::BlobDigest {
        let digest = eggwork_core::BlobDigest::from_bytes(bytes);
        blobs
            .put_stream(
                digest.clone(),
                bytes.len() as u64,
                stream::iter([Ok::<_, ()>(bytes::Bytes::from_static(bytes))]),
            )
            .await
            .unwrap();
        digest
    }

    #[tokio::test]
    async fn materializes_exact_tree_with_owner_permissions_and_fenced_resolution() {
        let temp = TempDir::new().unwrap();
        let blobs = BlobStore::open(temp.path().join("blobs"), 1024).unwrap();
        let manager = WorkspaceManager::open(temp.path().join("workspaces"), 1024).unwrap();
        let digest = upload_blob(&blobs, b"#!/bin/sh\necho hello\n").await;
        let manifest = WorkspaceManifest {
            schema_version: 1,
            entries: vec![
                WorkspaceEntry::Directory {
                    path: eggwork_core::RelativePath::new("src").unwrap(),
                },
                WorkspaceEntry::File {
                    path: eggwork_core::RelativePath::new("src/tool.sh").unwrap(),
                    digest,
                    size_bytes: 21,
                    executable: true,
                },
            ],
        };
        let handle = owner_handle("workspace-owner");
        let principal = PrincipalId::new("principal-a").unwrap();
        let workspace_id = WorkspaceId::new("ws-one").unwrap();
        let ready = manager
            .materialize(
                workspace_id.clone(),
                &handle,
                &principal,
                manifest.clone(),
                &blobs,
            )
            .await
            .unwrap();
        assert_eq!(
            fs::read(ready.root.join("src/tool.sh")).unwrap(),
            b"#!/bin/sh\necho hello\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(ready.root.join("src/tool.sh"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(ready.root.join("src"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        assert_eq!(
            manager
                .resolve(
                    &workspace_id,
                    &handle.execution_id,
                    handle.generation,
                    &principal,
                )
                .unwrap()
                .root,
            ready.root
        );
        assert!(
            manager
                .resolve(
                    &workspace_id,
                    &handle.execution_id,
                    handle.generation,
                    &PrincipalId::new("principal-b").unwrap(),
                )
                .is_err()
        );
        let duplicate = manager
            .materialize(workspace_id, &handle, &principal, manifest, &blobs)
            .await
            .unwrap();
        assert_eq!(duplicate.root, ready.root);
    }

    #[tokio::test]
    async fn missing_blobs_and_invalid_symlinks_never_create_ready_workspaces() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("workspaces");
        let blobs = BlobStore::open(temp.path().join("blobs"), 1024).unwrap();
        let manager = WorkspaceManager::open(&root, 1024).unwrap();
        let handle = owner_handle("workspace-failures");
        let principal = PrincipalId::new("principal-a").unwrap();
        let missing = WorkspaceManifest {
            schema_version: 1,
            entries: vec![WorkspaceEntry::File {
                path: eggwork_core::RelativePath::new("input").unwrap(),
                digest: eggwork_core::BlobDigest::from_bytes(b"not uploaded"),
                size_bytes: 12,
                executable: false,
            }],
        };
        assert!(matches!(
            manager
                .materialize(
                    WorkspaceId::new("missing-blob").unwrap(),
                    &handle,
                    &principal,
                    missing,
                    &blobs,
                )
                .await,
            Err(WorkspaceError::InvalidBlob)
        ));
        let symlink = WorkspaceManifest {
            schema_version: 1,
            entries: vec![WorkspaceEntry::Symlink {
                path: eggwork_core::RelativePath::new("escape").unwrap(),
                target: "../../outside".into(),
            }],
        };
        assert!(matches!(
            manager
                .materialize(
                    WorkspaceId::new("symlink").unwrap(),
                    &handle,
                    &principal,
                    symlink,
                    &blobs,
                )
                .await,
            Err(WorkspaceError::InvalidManifest)
        ));
        assert!(fs::read_dir(root).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("workspace-")
        }));
    }

    #[tokio::test]
    async fn concurrent_materialization_is_idempotent_and_recovery_cleans_unready_state() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("workspaces");
        let blobs = BlobStore::open(temp.path().join("blobs"), 1024).unwrap();
        let manager = WorkspaceManager::open(&root, 1024).unwrap();
        let digest = upload_blob(&blobs, b"same content").await;
        let manifest = WorkspaceManifest {
            schema_version: 1,
            entries: vec![WorkspaceEntry::File {
                path: eggwork_core::RelativePath::new("file.txt").unwrap(),
                digest,
                size_bytes: 12,
                executable: false,
            }],
        };
        let handle = owner_handle("workspace-race");
        let workspace_id = WorkspaceId::new("ws-race").unwrap();
        let principal = PrincipalId::new("principal-a").unwrap();
        let tasks = (0..12).map(|_| {
            let manager = manager.clone();
            let blobs = blobs.clone();
            let manifest = manifest.clone();
            let handle = handle.clone();
            let workspace_id = workspace_id.clone();
            let principal = principal.clone();
            async move {
                manager
                    .materialize(workspace_id, &handle, &principal, manifest, &blobs)
                    .await
                    .unwrap()
            }
        });
        let roots = join_all(tasks).await;
        assert!(
            roots
                .iter()
                .all(|workspace| workspace.root == roots[0].root)
        );
        drop(manager);
        let untracked = root.join(format!("workspace-{}", uuid::Uuid::new_v4()));
        let staged = root.join(".staging-crash");
        fs::create_dir(&untracked).unwrap();
        fs::create_dir(&staged).unwrap();
        drop(WorkspaceManager::open(&root, 1024).unwrap());
        assert!(!untracked.exists());
        assert!(!staged.exists());
    }

    #[tokio::test]
    async fn workspace_quota_is_checked_before_materialization() {
        let temp = TempDir::new().unwrap();
        let blobs = BlobStore::open(temp.path().join("blobs"), 1024).unwrap();
        let digest = upload_blob(&blobs, b"12345").await;
        let manager = WorkspaceManager::open(temp.path().join("workspaces"), 4).unwrap();
        let manifest = WorkspaceManifest {
            schema_version: 1,
            entries: vec![WorkspaceEntry::File {
                path: eggwork_core::RelativePath::new("file").unwrap(),
                digest,
                size_bytes: 5,
                executable: false,
            }],
        };
        assert!(matches!(
            manager
                .materialize(
                    WorkspaceId::new("ws-quota").unwrap(),
                    &owner_handle("workspace-quota"),
                    &PrincipalId::new("principal-a").unwrap(),
                    manifest,
                    &blobs,
                )
                .await,
            Err(WorkspaceError::QuotaExceeded)
        ));
    }
}
