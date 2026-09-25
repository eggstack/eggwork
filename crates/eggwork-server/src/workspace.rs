//! Validated, execution-owned workspace materialization.

use crate::blob::{BlobStore, MAX_BLOB_BYTES};
use eggwork_core::{
    BlobDigest, ExecutionGeneration, ExecutionHandle, ExecutionId, PrincipalId, WorkspaceEntry,
    WorkspaceId, WorkspaceManifest, WorkspaceManifestPatch,
};
use rusqlite::TransactionBehavior;
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
    #[error("workspace patch is invalid")]
    InvalidPatch,
    #[error("base manifest is missing or expired")]
    BaseMissing,
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
    #[error("workspace blob reference operation failed")]
    Blob(#[from] crate::blob::BlobError),
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
               created_unix_ms INTEGER NOT NULL,
               expires_unix_ms INTEGER
             );
             CREATE TABLE IF NOT EXISTS retained_manifests (
               principal_id TEXT NOT NULL,
               manifest_digest TEXT NOT NULL,
               manifest_json BLOB NOT NULL,
               logical_bytes INTEGER NOT NULL,
               created_unix_ms INTEGER NOT NULL,
               expires_unix_ms INTEGER,
               PRIMARY KEY (principal_id, manifest_digest)
             );
             CREATE INDEX IF NOT EXISTS retained_manifests_expiry
               ON retained_manifests(expires_unix_ms);",
        )?;
        let has_expiry = {
            let mut columns = metadata.prepare("PRAGMA table_info(workspaces)")?;
            columns
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<Result<Vec<_>, _>>()?
                .iter()
                .any(|name| name == "expires_unix_ms")
        };
        if !has_expiry {
            metadata.execute(
                "ALTER TABLE workspaces ADD COLUMN expires_unix_ms INTEGER",
                [],
            )?;
        }
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
                || existing
                    .expires_unix_ms
                    .is_some_and(|expires| expires <= crate::artifact::now_unix_ms())
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

        let digests = manifest
            .entries
            .iter()
            .filter_map(|entry| match entry {
                WorkspaceEntry::File { digest, .. } => Some(digest.clone()),
                _ => None,
            })
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let initial_expiry = crate::artifact::now_unix_ms()
            .saturating_add(crate::artifact::DEFAULT_RETENTION_MILLIS);
        blobs.retain(
            "workspace",
            workspace_id.as_str(),
            &digests,
            Some(initial_expiry),
        )?;

        // A successful create registers its canonical manifest as a
        // same-principal derived base before the tree becomes visible. The
        // registration only pins already-verified blobs for the bounded
        // retention horizon; a tree failure below leaves a harmless expiring
        // cache entry, never a ready workspace.
        self.store_manifest(principal_id, &manifest, &manifest_digest, blobs)?;

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
                let _ = blobs.release_references("workspace", workspace_id.as_str());
                return Err(WorkspaceError::Worker);
            }
        };
        if let Err(error) = result {
            let _ = fs::remove_dir_all(&stage);
            let _ = blobs.release_references("workspace", workspace_id.as_str());
            return Err(error);
        }
        if let Err(error) = fs::rename(&stage, &target) {
            let _ = fs::remove_dir_all(&stage);
            let _ = blobs.release_references("workspace", workspace_id.as_str());
            return Err(error.into());
        }
        if let Err(error) = sync_directory(&self.inner.root) {
            let _ = fs::remove_dir_all(&target);
            let _ = blobs.release_references("workspace", workspace_id.as_str());
            return Err(error.into());
        }
        let inserted = match self.inner.metadata.lock() {
            Ok(connection) => connection.execute(
                "INSERT INTO workspaces(workspace_id, execution_id, generation, principal_id,
                   manifest_digest, storage_key, logical_bytes, state, created_unix_ms, expires_unix_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'Ready', ?8, ?9)",
                params![
                    workspace_id.as_str(),
                    handle.execution_id.as_str(),
                    handle.generation.get() as i64,
                    principal_id.as_str(),
                    manifest_digest,
                    storage_key,
                    logical_bytes as i64,
                    crate::blob::unix_millis(),
                    initial_expiry as i64,
                ],
            ),
            Err(_) => {
                let _ = fs::remove_dir_all(&target);
                let _ = blobs.release_references("workspace", workspace_id.as_str());
                return Err(WorkspaceError::Worker);
            }
        };
        if let Err(error) = inserted {
            let _ = fs::remove_dir_all(&target);
            let _ = blobs.release_references("workspace", workspace_id.as_str());
            return Err(WorkspaceError::Metadata(error));
        }
        Ok(ReadyWorkspace {
            root: target,
            logical_bytes,
            manifest_digest,
        })
    }

    /// Create a fresh execution-private workspace from a retained canonical
    /// base manifest plus a bounded deterministic patch.
    ///
    /// The base lookup is scoped to `(principal_id, base_manifest_digest)`,
    /// so a missing/expired base is indistinguishable from a base retained
    /// for another principal. A miss has zero ready-workspace side effect.
    /// The final tree passes through the existing validator and the existing
    /// materialization path, so the result is byte-identical to submitting
    /// the equivalent full manifest.
    pub async fn materialize_derived(
        &self,
        workspace_id: WorkspaceId,
        handle: &ExecutionHandle,
        principal_id: &PrincipalId,
        patch: WorkspaceManifestPatch,
        blobs: &BlobStore,
    ) -> Result<ReadyWorkspace, WorkspaceError> {
        patch.validate().map_err(|_| WorkspaceError::InvalidPatch)?;
        let Some(base) = self.lookup_manifest(principal_id, &patch.base_manifest_digest, blobs)?
        else {
            return Err(WorkspaceError::BaseMissing);
        };
        let final_manifest = patch
            .apply_to(&base)
            .map_err(|_| WorkspaceError::InvalidManifest)?;
        self.materialize(workspace_id, handle, principal_id, final_manifest, blobs)
            .await
    }

    /// Persist a validated canonical manifest as a derived base for later
    /// same-principal patches, pinning its blobs under a distinct
    /// `manifest` owner kind for the bounded retention horizon. Re-registration
    /// extends expiry monotonically; it never shortens it.
    fn store_manifest(
        &self,
        principal_id: &PrincipalId,
        manifest: &WorkspaceManifest,
        manifest_digest: &str,
        blobs: &BlobStore,
    ) -> Result<(), WorkspaceError> {
        manifest
            .validate()
            .map_err(|_| WorkspaceError::InvalidManifest)?;
        BlobDigest::parse(manifest_digest.to_owned())
            .map_err(|_| WorkspaceError::InvalidManifest)?;
        let encoded = serde_json::to_vec(manifest)?;
        let logical_bytes = manifest.logical_bytes();
        let now = crate::artifact::now_unix_ms();
        let fresh_expiry = now.saturating_add(crate::artifact::DEFAULT_RETENTION_MILLIS);
        let connection = self
            .inner
            .metadata
            .lock()
            .map_err(|_| WorkspaceError::Worker)?;
        let existing: Option<Option<i64>> = connection
            .query_row(
                "SELECT expires_unix_ms FROM retained_manifests
                 WHERE principal_id = ?1 AND manifest_digest = ?2",
                params![principal_id.as_str(), manifest_digest],
                |row| row.get(0),
            )
            .optional()?;
        let expires = existing
            .flatten()
            .map(|value| (value.max(0) as u64).max(fresh_expiry))
            .unwrap_or(fresh_expiry);
        connection.execute(
            "INSERT INTO retained_manifests(principal_id, manifest_digest, manifest_json,
               logical_bytes, created_unix_ms, expires_unix_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(principal_id, manifest_digest) DO UPDATE SET
               manifest_json = excluded.manifest_json,
               logical_bytes = excluded.logical_bytes,
               expires_unix_ms = excluded.expires_unix_ms",
            params![
                principal_id.as_str(),
                manifest_digest,
                encoded,
                logical_bytes as i64,
                now as i64,
                expires as i64,
            ],
        )?;
        drop(connection);
        let digests = manifest
            .entries
            .iter()
            .filter_map(|entry| match entry {
                WorkspaceEntry::File { digest, .. } => Some(digest.clone()),
                _ => None,
            })
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        blobs.retain(
            "manifest",
            &manifest_owner_id(principal_id, manifest_digest),
            &digests,
            Some(expires),
        )?;
        Ok(())
    }

    /// Resolve a retained canonical manifest within one principal's scope.
    /// Expired bases behave as missing. A record whose blobs are no longer
    /// available is reaped and reported as missing; it is never usable.
    pub fn lookup_manifest(
        &self,
        principal_id: &PrincipalId,
        manifest_digest: &BlobDigest,
        blobs: &BlobStore,
    ) -> Result<Option<WorkspaceManifest>, WorkspaceError> {
        let digest = manifest_digest.as_str();
        let row: Option<(Vec<u8>, Option<i64>)> = self
            .inner
            .metadata
            .lock()
            .map_err(|_| WorkspaceError::Worker)?
            .query_row(
                "SELECT manifest_json, expires_unix_ms FROM retained_manifests
                 WHERE principal_id = ?1 AND manifest_digest = ?2",
                params![principal_id.as_str(), digest],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((encoded, expires)) = row else {
            return Ok(None);
        };
        if expires.is_some_and(|value| (value.max(0) as u64) <= crate::artifact::now_unix_ms()) {
            return Ok(None);
        }
        let manifest: WorkspaceManifest =
            serde_json::from_slice(&encoded).map_err(|_| WorkspaceError::InvalidManifest)?;
        if manifest.validate().is_err() {
            self.drop_manifest(principal_id, digest, blobs);
            return Ok(None);
        }
        for entry in &manifest.entries {
            if let WorkspaceEntry::File {
                digest: blob_digest,
                size_bytes,
                ..
            } = entry
            {
                match blobs.stored_size(blob_digest) {
                    Ok(size) if size == *size_bytes => {}
                    _ => {
                        self.drop_manifest(principal_id, digest, blobs);
                        return Ok(None);
                    }
                }
            }
        }
        Ok(Some(manifest))
    }

    fn drop_manifest(&self, principal_id: &PrincipalId, manifest_digest: &str, blobs: &BlobStore) {
        if let Ok(connection) = self.inner.metadata.lock() {
            let _ = connection.execute(
                "DELETE FROM retained_manifests WHERE principal_id = ?1 AND manifest_digest = ?2",
                params![principal_id.as_str(), manifest_digest],
            );
        }
        let _ = blobs.release_references(
            "manifest",
            &manifest_owner_id(principal_id, manifest_digest),
        );
    }

    /// Release expired retained manifests and their blob references. The pass
    /// is bounded by `limit`; callers repeat it to drain larger backlogs.
    /// Returns `(expired_candidates, manifests_deleted)`.
    pub fn manifest_garbage_collect(
        &self,
        now_unix_ms: u64,
        limit: usize,
        dry_run: bool,
        blobs: &BlobStore,
    ) -> Result<(u64, u64), WorkspaceError> {
        let connection = self
            .inner
            .metadata
            .lock()
            .map_err(|_| WorkspaceError::Worker)?;
        let expired: Vec<(String, String)> = {
            let mut statement = connection.prepare(
                "SELECT principal_id, manifest_digest FROM retained_manifests
                 WHERE expires_unix_ms IS NOT NULL AND expires_unix_ms <= ?1
                 ORDER BY expires_unix_ms ASC LIMIT ?2",
            )?;
            statement
                .query_map(
                    params![now_unix_ms as i64, limit.min(i64::MAX as usize) as i64],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?
                .collect::<Result<_, _>>()?
        };
        let candidates = expired.len() as u64;
        if dry_run {
            return Ok((candidates, 0));
        }
        let mut deleted = 0u64;
        for (principal_id, manifest_digest) in &expired {
            connection.execute(
                "DELETE FROM retained_manifests
                 WHERE principal_id = ?1 AND manifest_digest = ?2
                   AND expires_unix_ms IS NOT NULL AND expires_unix_ms <= ?3",
                params![principal_id, manifest_digest, now_unix_ms as i64],
            )?;
            let _ =
                blobs.release_references("manifest", &format!("{principal_id}:{manifest_digest}"));
            deleted += 1;
        }
        Ok((candidates, deleted))
    }

    /// Crash-safety: manifests created before expiry tracking must still
    /// expire instead of pinning blobs forever.
    pub fn recover_manifest_retention(&self, expires_unix_ms: u64) -> Result<(), WorkspaceError> {
        self.inner
            .metadata
            .lock()
            .map_err(|_| WorkspaceError::Worker)?
            .execute(
                "UPDATE retained_manifests SET expires_unix_ms = ?1
                 WHERE expires_unix_ms IS NULL",
                [expires_unix_ms as i64],
            )?;
        Ok(())
    }

    /// Restart reconciliation for the manifest cache. Each step is bounded by
    /// `limit` so a large stale cache cannot stall startup; leftovers are
    /// reaped by the next restart or GC pass. Never leaves permanent
    /// unbounded blob references behind.
    pub fn reconcile_manifests(
        &self,
        blobs: &BlobStore,
        now_unix_ms: u64,
        limit: usize,
    ) -> Result<(), WorkspaceError> {
        let limit = limit.clamp(1, 1024);
        let rows: Vec<(String, String, Vec<u8>)> = {
            let connection = self
                .inner
                .metadata
                .lock()
                .map_err(|_| WorkspaceError::Worker)?;
            let mut statement = connection.prepare(
                "SELECT principal_id, manifest_digest, manifest_json FROM retained_manifests
                 ORDER BY principal_id, manifest_digest LIMIT ?1",
            )?;
            statement
                .query_map([limit.min(i64::MAX as usize) as i64], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?
                .collect::<Result<_, _>>()?
        };
        for (principal_id, manifest_digest, encoded) in &rows {
            let principal = PrincipalId::new(principal_id.clone());
            let digest_valid = BlobDigest::parse(manifest_digest.clone()).is_ok();
            let body_valid = principal.is_ok()
                && serde_json::from_slice::<WorkspaceManifest>(encoded)
                    .is_ok_and(|manifest| manifest.validate().is_ok());
            if !digest_valid || !body_valid {
                if let Ok(principal) = principal {
                    self.drop_manifest(&principal, manifest_digest, blobs);
                } else if let Ok(connection) = self.inner.metadata.lock() {
                    let _ = connection.execute(
                        "DELETE FROM retained_manifests
                         WHERE principal_id = ?1 AND manifest_digest = ?2",
                        params![principal_id, manifest_digest],
                    );
                    let _ = blobs.release_references(
                        "manifest",
                        &format!("{principal_id}:{manifest_digest}"),
                    );
                }
            }
        }
        let _ = self.manifest_garbage_collect(now_unix_ms, limit, false, blobs)?;
        // Orphan `manifest` blob references (no matching cache row, e.g. after
        // a crash between reference creation and row commit) must expire
        // instead of pinning bytes indefinitely.
        let known: HashSet<String> = {
            let connection = self
                .inner
                .metadata
                .lock()
                .map_err(|_| WorkspaceError::Worker)?;
            let mut statement = connection
                .prepare("SELECT principal_id, manifest_digest FROM retained_manifests")?;
            statement
                .query_map([], |row| {
                    Ok(format!(
                        "{}:{}",
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?
                    ))
                })?
                .collect::<Result<_, _>>()?
        };
        let orphans = blobs
            .reference_owners("manifest", limit)
            .map_err(WorkspaceError::from)?;
        for owner in orphans.iter().take(limit) {
            if !known.contains(owner) {
                let _ = blobs.release_references("manifest", owner);
            }
        }
        Ok(())
    }

    pub fn mark_terminal(
        &self,
        workspace_id: &WorkspaceId,
        expires_unix_ms: u64,
        blobs: &BlobStore,
    ) -> Result<(), WorkspaceError> {
        self.inner
            .metadata
            .lock()
            .map_err(|_| WorkspaceError::Worker)?
            .execute(
                "UPDATE workspaces SET expires_unix_ms = ?2 WHERE workspace_id = ?1",
                params![workspace_id.as_str(), expires_unix_ms as i64],
            )?;
        blobs.set_reference_expiry("workspace", workspace_id.as_str(), expires_unix_ms)?;
        Ok(())
    }

    pub fn mark_active(
        &self,
        workspace_id: &WorkspaceId,
        blobs: &BlobStore,
    ) -> Result<(), WorkspaceError> {
        let mut connection = self
            .inner
            .metadata
            .lock()
            .map_err(|_| WorkspaceError::Worker)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let key: Option<String> = transaction
            .query_row(
                "SELECT storage_key FROM workspaces WHERE workspace_id = ?1 AND state = 'Ready'
                 AND (expires_unix_ms IS NULL OR expires_unix_ms > ?2)",
                params![workspace_id.as_str(), crate::artifact::now_unix_ms() as i64],
                |row| row.get(0),
            )
            .optional()?;
        let Some(key) = key.filter(|key| valid_storage_key(key)) else {
            return Err(WorkspaceError::NotFound);
        };
        let root = self.inner.root.join(key);
        let metadata = fs::symlink_metadata(root)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(WorkspaceError::NotFound);
        }
        transaction.execute(
            "UPDATE workspaces SET expires_unix_ms = NULL WHERE workspace_id = ?1",
            [workspace_id.as_str()],
        )?;
        transaction.commit()?;
        blobs.clear_reference_expiry("workspace", workspace_id.as_str())?;
        Ok(())
    }

    pub fn recover_retention(
        &self,
        expires_unix_ms: u64,
        blobs: &BlobStore,
    ) -> Result<(), WorkspaceError> {
        let ids = {
            let connection = self
                .inner
                .metadata
                .lock()
                .map_err(|_| WorkspaceError::Worker)?;
            let mut statement = connection
                .prepare("SELECT workspace_id FROM workspaces WHERE expires_unix_ms IS NULL")?;
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        self.inner
            .metadata
            .lock()
            .map_err(|_| WorkspaceError::Worker)?
            .execute(
                "UPDATE workspaces SET expires_unix_ms = ?1 WHERE expires_unix_ms IS NULL",
                [expires_unix_ms as i64],
            )?;
        for id in ids {
            blobs.set_reference_expiry("workspace", &id, expires_unix_ms)?;
        }
        Ok(())
    }

    pub fn garbage_collect(
        &self,
        now_unix_ms: u64,
        limit: usize,
        dry_run: bool,
    ) -> Result<(u64, u64, u64), WorkspaceError> {
        let mut connection = self
            .inner
            .metadata
            .lock()
            .map_err(|_| WorkspaceError::Worker)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let candidates: Vec<(String, String, u64)> = {
            let mut statement = transaction.prepare(
                "SELECT workspace_id, storage_key, logical_bytes FROM workspaces
                 WHERE expires_unix_ms IS NOT NULL AND expires_unix_ms <= ?1
                 ORDER BY expires_unix_ms ASC LIMIT ?2",
            )?;
            statement
                .query_map(
                    params![now_unix_ms as i64, limit.min(i64::MAX as usize) as i64],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get::<_, i64>(2)?.max(0) as u64,
                        ))
                    },
                )?
                .collect::<Result<_, _>>()?
        };
        let bytes = candidates
            .iter()
            .fold(0u64, |sum, (_, _, size)| sum.saturating_add(*size));
        let mut deleted = 0u64;
        if !dry_run {
            for (workspace_id, key, _) in &candidates {
                if !valid_storage_key(key) {
                    continue;
                }
                match fs::remove_dir_all(self.inner.root.join(key)) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(WorkspaceError::Io(error)),
                }
                transaction.execute(
                    "DELETE FROM workspaces WHERE workspace_id = ?1
                         AND expires_unix_ms IS NOT NULL AND expires_unix_ms <= ?2",
                    params![workspace_id, now_unix_ms as i64],
                )?;
                deleted += 1;
            }
        }
        transaction.commit()?;
        Ok((candidates.len() as u64, bytes, deleted))
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
            || record
                .expires_unix_ms
                .is_some_and(|expires| expires <= crate::artifact::now_unix_ms())
        {
            return Err(WorkspaceError::NotFound);
        }
        if !valid_storage_key(&record.storage_key) {
            return Err(WorkspaceError::NotFound);
        }
        let root = self.inner.root.join(record.storage_key);
        let metadata = fs::symlink_metadata(&root)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
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
                   logical_bytes, state, expires_unix_ms FROM workspaces WHERE workspace_id = ?1",
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
                        expires_unix_ms: row.get::<_, Option<i64>>(7)?.map(|value| value as u64),
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
    expires_unix_ms: Option<u64>,
}

fn valid_storage_key(key: &str) -> bool {
    key.strip_prefix("workspace-")
        .and_then(|value| uuid::Uuid::parse_str(value).ok())
        .is_some()
}

fn manifest_owner_id(principal_id: &PrincipalId, manifest_digest: &str) -> String {
    format!("{}:{manifest_digest}", principal_id.as_str())
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

    #[tokio::test]
    async fn terminal_retention_gc_preserves_live_workspace_then_reclaims_inputs() {
        let temp = TempDir::new().unwrap();
        let blobs = BlobStore::open(temp.path().join("blobs"), 1024).unwrap();
        let digest = upload_blob(&blobs, b"retained input").await;
        let manager = WorkspaceManager::open(temp.path().join("workspaces"), 1024).unwrap();
        let workspace_id = WorkspaceId::new("ws-retention").unwrap();
        let manifest = WorkspaceManifest {
            schema_version: 1,
            entries: vec![WorkspaceEntry::File {
                path: eggwork_core::RelativePath::new("input.txt").unwrap(),
                digest: digest.clone(),
                size_bytes: 14,
                executable: false,
            }],
        };
        let ready = manager
            .materialize(
                workspace_id.clone(),
                &owner_handle("workspace-retention"),
                &PrincipalId::new("principal-a").unwrap(),
                manifest,
                &blobs,
            )
            .await
            .unwrap();
        let now = crate::artifact::now_unix_ms();
        let active_gc = blobs.garbage_collect(now, 8, false).await.unwrap();
        assert_eq!(active_gc.removed_blobs, 0);
        assert!(ready.root.exists());

        manager.mark_terminal(&workspace_id, 1_000, &blobs).unwrap();
        let dry = manager.garbage_collect(1_000, 1, true).unwrap();
        assert_eq!(dry.0, 1);
        assert!(ready.root.exists());
        assert!(blobs.open_verified(&digest).await.is_ok());

        let deleted = manager.garbage_collect(1_000, 1, false).unwrap();
        assert_eq!(deleted.0, 1);
        assert!(!ready.root.exists());
        // The retained canonical manifest still pins the blob for its bounded
        // horizon, so blob reclaim waits for manifest expiry.
        let retained = blobs.garbage_collect(1_000, 1, false).await.unwrap();
        assert_eq!(retained.removed_blobs, 0);
        assert!(blobs.open_verified(&digest).await.is_ok());
        let manifest_expiry = crate::artifact::now_unix_ms()
            .saturating_add(crate::artifact::DEFAULT_RETENTION_MILLIS);
        let (candidates, manifests) = manager
            .manifest_garbage_collect(manifest_expiry + 1, 8, false, &blobs)
            .unwrap();
        assert_eq!((candidates, manifests), (1, 1));
        let reclaimed = blobs
            .garbage_collect(manifest_expiry + 1, 1, false)
            .await
            .unwrap();
        assert_eq!(reclaimed.removed_blobs, 1);
        assert!(blobs.open_verified(&digest).await.is_err());
    }

    fn derived_fixture_manifest(digest: eggwork_core::BlobDigest, size: u64) -> WorkspaceManifest {
        WorkspaceManifest {
            schema_version: 1,
            entries: vec![
                WorkspaceEntry::Directory {
                    path: eggwork_core::RelativePath::new("src").unwrap(),
                },
                WorkspaceEntry::File {
                    path: eggwork_core::RelativePath::new("src/base.txt").unwrap(),
                    digest,
                    size_bytes: size,
                    executable: false,
                },
            ],
        }
    }

    fn derived_patch(
        base: &eggwork_core::BlobDigest,
        entries: Vec<eggwork_core::WorkspacePatchEntry>,
    ) -> eggwork_core::WorkspaceManifestPatch {
        eggwork_core::WorkspaceManifestPatch {
            schema_version: 1,
            base_manifest_digest: base.clone(),
            entries,
        }
    }

    #[tokio::test]
    async fn full_create_registers_base_and_same_principal_derived_hit_matches_full_digest() {
        let temp = TempDir::new().unwrap();
        let blobs = BlobStore::open(temp.path().join("blobs"), 1024 * 1024).unwrap();
        let manager = WorkspaceManager::open(temp.path().join("workspaces"), 1024 * 1024).unwrap();
        let principal = PrincipalId::new("principal-a").unwrap();
        let base_bytes = b"base content";
        let base_digest = upload_blob(&blobs, base_bytes).await;
        let base_manifest = derived_fixture_manifest(base_digest.clone(), base_bytes.len() as u64);
        let base_canonical = base_manifest.digest().unwrap();
        manager
            .materialize(
                WorkspaceId::new("ws-base").unwrap(),
                &owner_handle("exec-base"),
                &principal,
                base_manifest,
                &blobs,
            )
            .await
            .unwrap();
        // Full create registered the canonical manifest as a derived base.
        assert!(
            manager
                .lookup_manifest(&principal, &base_canonical, &blobs)
                .unwrap()
                .is_some()
        );

        let updated_bytes = b"updated content";
        let updated_digest = upload_blob(&blobs, updated_bytes).await;
        let patch = derived_patch(
            &base_canonical,
            vec![eggwork_core::WorkspacePatchEntry::File {
                path: eggwork_core::RelativePath::new("src/base.txt").unwrap(),
                digest: updated_digest.clone(),
                size_bytes: updated_bytes.len() as u64,
                executable: false,
            }],
        );
        let derived = manager
            .materialize_derived(
                WorkspaceId::new("ws-derived").unwrap(),
                &owner_handle("exec-derived"),
                &principal,
                patch,
                &blobs,
            )
            .await
            .unwrap();
        let expected = derived_fixture_manifest(updated_digest, updated_bytes.len() as u64);
        assert_eq!(derived.manifest_digest, expected.digest().unwrap().as_str());
        assert_eq!(
            fs::read(derived.root.join("src/base.txt")).unwrap(),
            updated_bytes
        );
        // The derived result is itself registered for further derivation.
        assert!(
            manager
                .lookup_manifest(
                    &principal,
                    &eggwork_core::BlobDigest::parse(derived.manifest_digest.clone()).unwrap(),
                    &blobs,
                )
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn cross_principal_lookup_behaves_as_missing_with_no_side_effect() {
        let temp = TempDir::new().unwrap();
        let blobs = BlobStore::open(temp.path().join("blobs"), 1024).unwrap();
        let manager = WorkspaceManager::open(temp.path().join("workspaces"), 1024).unwrap();
        let owner = PrincipalId::new("principal-a").unwrap();
        let other = PrincipalId::new("principal-b").unwrap();
        let digest = upload_blob(&blobs, b"owned").await;
        let manifest = derived_fixture_manifest(digest, 5);
        let canonical = manifest.digest().unwrap();
        manager
            .materialize(
                WorkspaceId::new("ws-owned").unwrap(),
                &owner_handle("exec-owned"),
                &owner,
                manifest,
                &blobs,
            )
            .await
            .unwrap();
        assert!(
            manager
                .lookup_manifest(&other, &canonical, &blobs)
                .unwrap()
                .is_none()
        );
        let patch = derived_patch(&canonical, vec![]);
        assert!(matches!(
            manager
                .materialize_derived(
                    WorkspaceId::new("ws-foreign").unwrap(),
                    &owner_handle("exec-foreign"),
                    &other,
                    patch,
                    &blobs,
                )
                .await,
            Err(WorkspaceError::BaseMissing)
        ));
        // The miss created no workspace directory or metadata row.
        assert!(
            manager
                .resolve(
                    &WorkspaceId::new("ws-foreign").unwrap(),
                    &ExecutionId::new("exec-foreign").unwrap(),
                    ExecutionGeneration::new(1).unwrap(),
                    &other,
                )
                .is_err()
        );
    }

    #[tokio::test]
    async fn expired_base_behaves_as_missing_and_gc_releases_pinned_blobs() {
        let temp = TempDir::new().unwrap();
        let blobs = BlobStore::open(temp.path().join("blobs"), 1024).unwrap();
        let manager = WorkspaceManager::open(temp.path().join("workspaces"), 1024).unwrap();
        let principal = PrincipalId::new("principal-a").unwrap();
        let digest = upload_blob(&blobs, b"pinned bytes").await;
        let manifest = derived_fixture_manifest(digest.clone(), 12);
        let canonical = manifest.digest().unwrap();
        let ready = manager
            .materialize(
                WorkspaceId::new("ws-pin").unwrap(),
                &owner_handle("exec-pin"),
                &principal,
                manifest,
                &blobs,
            )
            .await
            .unwrap();
        // Terminalize and collect the workspace: the retained manifest must
        // keep pinning the blob.
        manager
            .mark_terminal(&WorkspaceId::new("ws-pin").unwrap(), 1_000, &blobs)
            .unwrap();
        manager.garbage_collect(1_000, 8, false).unwrap();
        assert!(!ready.root.exists());
        assert!(blobs.open_verified(&digest).await.is_ok());
        assert!(
            manager
                .lookup_manifest(&principal, &canonical, &blobs)
                .unwrap()
                .is_some()
        );
        // Expire the manifest: lookups miss and GC releases the blob refs.
        let manifest_expiry = crate::artifact::now_unix_ms()
            .saturating_add(crate::artifact::DEFAULT_RETENTION_MILLIS);
        let (candidates, deleted) = manager
            .manifest_garbage_collect(manifest_expiry + 1, 8, false, &blobs)
            .unwrap();
        assert_eq!((candidates, deleted), (1, 1));
        assert!(
            manager
                .lookup_manifest(&principal, &canonical, &blobs)
                .unwrap()
                .is_none()
        );
        let patch = derived_patch(&canonical, vec![]);
        assert!(matches!(
            manager
                .materialize_derived(
                    WorkspaceId::new("ws-after-expiry").unwrap(),
                    &owner_handle("exec-after-expiry"),
                    &principal,
                    patch,
                    &blobs,
                )
                .await,
            Err(WorkspaceError::BaseMissing)
        ));
        let reclaimed = blobs
            .garbage_collect(manifest_expiry + 1, 8, false)
            .await
            .unwrap();
        assert_eq!(reclaimed.removed_blobs, 1);
        assert!(blobs.open_verified(&digest).await.is_err());
    }

    #[tokio::test]
    async fn manifest_missing_blob_is_reaped_and_crash_reopen_reconciles_bounds() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("workspaces");
        let blobs = BlobStore::open(temp.path().join("blobs"), 1024).unwrap();
        let manager = WorkspaceManager::open(&root, 1024).unwrap();
        let principal = PrincipalId::new("principal-a").unwrap();
        let digest = upload_blob(&blobs, b"vanishing").await;
        let manifest = derived_fixture_manifest(digest.clone(), 9);
        let canonical = manifest.digest().unwrap();
        manager
            .materialize(
                WorkspaceId::new("ws-vanish").unwrap(),
                &owner_handle("exec-vanish"),
                &principal,
                manifest,
                &blobs,
            )
            .await
            .unwrap();
        // Simulate blob loss beneath a retained manifest: the record must
        // become unusable and be reaped instead of resurrecting authority.
        blobs.release_references("workspace", "ws-vanish").unwrap();
        blobs
            .release_references(
                "manifest",
                &format!("{}:{}", principal.as_str(), canonical.as_str()),
            )
            .unwrap();
        let horizon = crate::artifact::now_unix_ms()
            .saturating_add(crate::artifact::DEFAULT_RETENTION_MILLIS);
        blobs.garbage_collect(horizon + 1, 8, false).await.unwrap();
        assert!(
            manager
                .lookup_manifest(&principal, &canonical, &blobs)
                .unwrap()
                .is_none()
        );
        drop(manager);
        let reopened = WorkspaceManager::open(&root, 1024).unwrap();
        reopened
            .reconcile_manifests(&blobs, crate::artifact::now_unix_ms(), 8)
            .unwrap();
        assert!(blobs.reference_owners("manifest", 8).unwrap().is_empty());
    }

    #[tokio::test]
    async fn concurrent_same_derived_materialization_is_idempotent_and_conflicts_fenced() {
        let temp = TempDir::new().unwrap();
        let blobs = BlobStore::open(temp.path().join("blobs"), 1024 * 1024).unwrap();
        let manager = WorkspaceManager::open(temp.path().join("workspaces"), 1024 * 1024).unwrap();
        let principal = PrincipalId::new("principal-a").unwrap();
        let digest = upload_blob(&blobs, b"shared base").await;
        let manifest = derived_fixture_manifest(digest, 11);
        let canonical = manifest.digest().unwrap();
        manager
            .materialize(
                WorkspaceId::new("ws-shared-base").unwrap(),
                &owner_handle("exec-shared-base"),
                &principal,
                manifest,
                &blobs,
            )
            .await
            .unwrap();
        let patch = || derived_patch(&canonical, vec![]);
        let tasks = (0..12).map(|_| {
            let manager = manager.clone();
            let blobs = blobs.clone();
            let principal = principal.clone();
            let patch = patch();
            async move {
                manager
                    .materialize_derived(
                        WorkspaceId::new("ws-derived-race").unwrap(),
                        &owner_handle("exec-derived-race"),
                        &principal,
                        patch,
                        &blobs,
                    )
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
        // Same workspace id with a different final digest conflicts.
        let new_digest = upload_blob(&blobs, b"different!").await;
        let conflicting = derived_patch(
            &canonical,
            vec![eggwork_core::WorkspacePatchEntry::File {
                path: eggwork_core::RelativePath::new("src/base.txt").unwrap(),
                digest: new_digest,
                size_bytes: 10,
                executable: false,
            }],
        );
        assert!(matches!(
            manager
                .materialize_derived(
                    WorkspaceId::new("ws-derived-race").unwrap(),
                    &owner_handle("exec-derived-race"),
                    &principal,
                    conflicting,
                    &blobs,
                )
                .await,
            Err(WorkspaceError::Conflict)
        ));
        // Malformed patches never create workspaces.
        let bad = eggwork_core::WorkspaceManifestPatch {
            schema_version: 1,
            base_manifest_digest: canonical.clone(),
            entries: vec![
                eggwork_core::WorkspacePatchEntry::Remove {
                    path: eggwork_core::RelativePath::new("src/base.txt").unwrap(),
                },
                eggwork_core::WorkspacePatchEntry::Remove {
                    path: eggwork_core::RelativePath::new("src/base.txt").unwrap(),
                },
            ],
        };
        assert!(matches!(
            manager
                .materialize_derived(
                    WorkspaceId::new("ws-bad-patch").unwrap(),
                    &owner_handle("exec-bad-patch"),
                    &principal,
                    bad,
                    &blobs,
                )
                .await,
            Err(WorkspaceError::InvalidPatch)
        ));
    }
}
