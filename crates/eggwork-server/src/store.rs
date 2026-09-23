//! Bounded durable execution and event journal owned by the node.

use eggwork_core::{
    EventMetadata, EventSequence, ExecutionEvent, ExecutionEventKind, ExecutionFailure,
    ExecutionGeneration, ExecutionId, ExecutionResult, ExecutionSnapshot, ExecutionState,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sha2::Digest;
use std::{
    fs::OpenOptions,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;

const MAX_RETAINED_EVENTS: usize = 256;
const MAX_RETAINED_EVENT_BYTES: usize = 512 * 1024;
const MAX_EXECUTION_IDENTITIES: i64 = 2048;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("execution store failed")]
    Sql(#[from] rusqlite::Error),
    #[error("execution store serialization failed")]
    Json(#[from] serde_json::Error),
    #[error("execution store worker failed")]
    Worker,
    #[error("execution event exceeds its protocol bound")]
    InvalidEvent,
}

#[derive(Clone)]
pub struct ExecutionStore {
    connection: Arc<Mutex<Connection>>,
}

#[derive(Debug)]
pub enum ReserveResult {
    Created,
    Existing(ExecutionSnapshot),
    Conflict,
    StaleGeneration,
    StorageFull,
}

#[derive(Debug)]
pub struct ExistingExecution {
    pub canonical_version: u16,
    pub digest: String,
    pub principal_id: String,
    pub lease_token_hash: String,
    pub snapshot: ExecutionSnapshot,
}

#[derive(Debug)]
pub struct EventPage {
    pub base_sequence: u64,
    pub next_sequence: u64,
    pub events: Vec<ExecutionEvent>,
}

impl ExecutionStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        if !path.as_ref().exists() {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let _file = options.open(path.as_ref()).map_err(|error| {
                StoreError::Sql(rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
            })?;
        }
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(2))?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS executions (
               execution_id TEXT NOT NULL,
               generation INTEGER NOT NULL,
               canonical_version INTEGER NOT NULL,
               digest TEXT NOT NULL,
               principal_id TEXT NOT NULL,
               lease_token_hash TEXT NOT NULL,
               lease_expires_unix_ms INTEGER NOT NULL,
               last_renewal_id TEXT,
               state TEXT NOT NULL,
               next_sequence INTEGER NOT NULL DEFAULT 1,
               snapshot_json BLOB NOT NULL,
               created_unix_ms INTEGER NOT NULL,
               updated_unix_ms INTEGER NOT NULL,
               PRIMARY KEY (execution_id, generation)
             );
             CREATE INDEX IF NOT EXISTS executions_latest
               ON executions(execution_id, generation DESC);
             CREATE TABLE IF NOT EXISTS execution_events (
               execution_id TEXT NOT NULL,
               generation INTEGER NOT NULL,
               sequence INTEGER NOT NULL,
               encoded BLOB NOT NULL,
               PRIMARY KEY (execution_id, generation, sequence),
               FOREIGN KEY (execution_id, generation)
                 REFERENCES executions(execution_id, generation) ON DELETE CASCADE
             );",
        )?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    pub async fn reserve(
        &self,
        snapshot: ExecutionSnapshot,
        canonical_version: u16,
        digest: String,
        principal_id: String,
        lease_token_hash: String,
        lease_expires_unix_ms: i64,
    ) -> Result<ReserveResult, StoreError> {
        let connection = self.connection.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = connection.lock().map_err(|_| StoreError::Worker)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let id = snapshot.execution_id.as_str();
            let generation = snapshot.generation.get() as i64;
            let existing: Option<(i64, String, String, String, Vec<u8>)> = transaction
                .query_row(
                    "SELECT canonical_version, digest, principal_id, lease_token_hash, snapshot_json FROM executions
                     WHERE execution_id = ?1 AND generation = ?2",
                    params![id, generation],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
                )
                .optional()?;
            if let Some((old_canonical_version, old_digest, old_principal, old_lease_hash, encoded)) = existing {
                let result = if old_canonical_version == i64::from(canonical_version)
                    && old_digest == digest
                    && old_principal == principal_id
                    && old_lease_hash == lease_token_hash
                {
                    ReserveResult::Existing(serde_json::from_slice(&encoded)?)
                } else {
                    ReserveResult::Conflict
                };
                transaction.commit()?;
                return Ok(result);
            }

            let latest: Option<(i64, String)> = transaction
                .query_row(
                    "SELECT generation, state FROM executions
                     WHERE execution_id = ?1 ORDER BY generation DESC LIMIT 1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            match latest {
                None if generation != 1 => return Ok(ReserveResult::StaleGeneration),
                Some((latest_generation, latest_state)) => {
                    if generation != latest_generation + 1
                        || !matches!(
                            latest_state.as_str(),
                            "Succeeded" | "Failed" | "Cancelled" | "TimedOut" | "Interrupted"
                        )
                    {
                        return Ok(ReserveResult::StaleGeneration);
                    }
                }
                _ => {}
            }

            let execution_count: i64 =
                transaction.query_row("SELECT COUNT(*) FROM executions", [], |row| row.get(0))?;
            if execution_count >= MAX_EXECUTION_IDENTITIES {
                return Ok(ReserveResult::StorageFull);
            }

            let encoded = serde_json::to_vec(&snapshot)?;
            let now = unix_millis();
            transaction.execute(
                "INSERT INTO executions (
                   execution_id, generation, canonical_version, digest, principal_id, lease_token_hash,
                   lease_expires_unix_ms, state, next_sequence, snapshot_json,
                   created_unix_ms, updated_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?10, ?10)",
                params![
                    id,
                    generation,
                    canonical_version,
                    digest,
                    principal_id,
                    lease_token_hash,
                    lease_expires_unix_ms,
                    state_name(&snapshot.state),
                    encoded,
                    now,
                ],
            )?;
            transaction.commit()?;
            Ok(ReserveResult::Created)
        })
        .await
        .map_err(|_| StoreError::Worker)?
    }

    pub async fn lookup(
        &self,
        id: ExecutionId,
        generation: ExecutionGeneration,
    ) -> Result<Option<ExistingExecution>, StoreError> {
        let connection = self.connection.clone();
        tokio::task::spawn_blocking(move || {
            let connection = connection.lock().map_err(|_| StoreError::Worker)?;
            let row: Option<(i64, String, String, String, Vec<u8>)> = connection
                .query_row(
                    "SELECT canonical_version, digest, principal_id, lease_token_hash, snapshot_json
                     FROM executions WHERE execution_id = ?1 AND generation = ?2",
                    params![id.as_str(), generation.get() as i64],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
                )
                .optional()?;
            row.map(
                |(canonical_version, digest, principal_id, lease_token_hash, snapshot)| {
                    Ok(ExistingExecution {
                        canonical_version: canonical_version as u16,
                        digest,
                        principal_id,
                        lease_token_hash,
                        snapshot: serde_json::from_slice(&snapshot)?,
                    })
                },
            )
            .transpose()
        })
        .await
        .map_err(|_| StoreError::Worker)?
    }

    /// Load a snapshot only when the authenticated principal owns that
    /// execution generation. A foreign execution is indistinguishable from a
    /// missing one at the observe boundary.
    pub async fn load_snapshot_for_principal(
        &self,
        id: ExecutionId,
        generation: Option<ExecutionGeneration>,
        principal_id: String,
    ) -> Result<Option<ExecutionSnapshot>, StoreError> {
        let connection = self.connection.clone();
        tokio::task::spawn_blocking(move || {
            let connection = connection.lock().map_err(|_| StoreError::Worker)?;
            let row: Option<Vec<u8>> = match generation {
                Some(generation) => connection
                    .query_row(
                        "SELECT snapshot_json FROM executions
                         WHERE execution_id = ?1 AND generation = ?2 AND principal_id = ?3",
                        params![id.as_str(), generation.get() as i64, principal_id],
                        |row| row.get(0),
                    )
                    .optional()?,
                None => connection
                    .query_row(
                        "SELECT snapshot_json FROM executions
                         WHERE execution_id = ?1 AND principal_id = ?2
                         ORDER BY generation DESC LIMIT 1",
                        params![id.as_str(), principal_id],
                        |row| row.get(0),
                    )
                    .optional()?,
            };
            row.map(|encoded| serde_json::from_slice(&encoded).map_err(StoreError::from))
                .transpose()
        })
        .await
        .map_err(|_| StoreError::Worker)?
    }

    pub async fn commit_event(
        &self,
        snapshot: ExecutionSnapshot,
        kind: ExecutionEventKind,
    ) -> Result<Option<ExecutionEvent>, StoreError> {
        let connection = self.connection.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = connection.lock().map_err(|_| StoreError::Worker)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let id = snapshot.execution_id.as_str();
            let generation = snapshot.generation.get() as i64;
            let current: Option<(String, i64)> = transaction
                .query_row(
                    "SELECT state, next_sequence FROM executions
                     WHERE execution_id = ?1 AND generation = ?2",
                    params![id, generation],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let Some((state, sequence)) = current else {
                transaction.rollback()?;
                return Ok(None);
            };
            if is_terminal_name(&state) {
                transaction.rollback()?;
                return Ok(None);
            }
            let event = ExecutionEvent {
                sequence: EventSequence::new(sequence as u64),
                kind,
                metadata: EventMetadata { fields: vec![] },
            };
            event.validate().map_err(|_| StoreError::InvalidEvent)?;
            let encoded = serde_json::to_vec(&event)?;
            if encoded.len() > MAX_RETAINED_EVENT_BYTES {
                return Err(StoreError::InvalidEvent);
            }
            let now = unix_millis();
            transaction.execute(
                "UPDATE executions SET state = ?3, next_sequence = ?4,
                   snapshot_json = ?5, updated_unix_ms = ?6
                 WHERE execution_id = ?1 AND generation = ?2",
                params![
                    id,
                    generation,
                    state_name(&snapshot.state),
                    sequence + 1,
                    serde_json::to_vec(&snapshot)?,
                    now,
                ],
            )?;
            transaction.execute(
                "INSERT INTO execution_events(execution_id, generation, sequence, encoded)
                 VALUES (?1, ?2, ?3, ?4)",
                params![id, generation, sequence, encoded],
            )?;
            trim_events(&transaction, id, generation)?;
            transaction.commit()?;
            Ok(Some(event))
        })
        .await
        .map_err(|_| StoreError::Worker)?
    }

    pub async fn update_lease(
        &self,
        id: ExecutionId,
        generation: ExecutionGeneration,
        lease_token_hash: String,
        renewal_id: String,
        lease_expires_unix_ms: i64,
    ) -> Result<Option<i64>, StoreError> {
        let connection = self.connection.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = connection.lock().map_err(|_| StoreError::Worker)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current: Option<(String, i64, Option<String>, String)> = transaction
                .query_row(
                    "SELECT lease_token_hash, lease_expires_unix_ms, last_renewal_id, state
                     FROM executions WHERE execution_id = ?1 AND generation = ?2",
                    params![id.as_str(), generation.get() as i64],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()?;
            let Some((current_hash, current_expiry, last_renewal_id, state)) = current else {
                transaction.rollback()?;
                return Ok(None);
            };
            if current_hash != lease_token_hash
                || is_terminal_name(&state)
                || current_expiry <= unix_millis()
            {
                transaction.rollback()?;
                return Ok(None);
            }
            if last_renewal_id.as_deref() == Some(renewal_id.as_str()) {
                transaction.commit()?;
                return Ok(Some(current_expiry));
            }
            let expires = current_expiry.max(lease_expires_unix_ms);
            transaction.execute(
                "UPDATE executions SET lease_expires_unix_ms = ?4, last_renewal_id = ?5,
                   updated_unix_ms = ?6 WHERE execution_id = ?1 AND generation = ?2
                   AND lease_token_hash = ?3",
                params![
                    id.as_str(),
                    generation.get() as i64,
                    lease_token_hash,
                    expires,
                    renewal_id,
                    unix_millis(),
                ],
            )?;
            transaction.commit()?;
            Ok(Some(expires))
        })
        .await
        .map_err(|_| StoreError::Worker)?
    }

    pub async fn load_page(
        &self,
        id: ExecutionId,
        generation: ExecutionGeneration,
        after_sequence: u64,
    ) -> Result<Option<EventPage>, StoreError> {
        let connection = self.connection.clone();
        tokio::task::spawn_blocking(move || {
            let connection = connection.lock().map_err(|_| StoreError::Worker)?;
            let exists: Option<i64> = connection
                .query_row(
                    "SELECT next_sequence FROM executions WHERE execution_id = ?1 AND generation = ?2",
                    params![id.as_str(), generation.get() as i64],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(next_sequence) = exists else {
                return Ok(None);
            };
            let base_sequence: Option<i64> = connection.query_row(
                "SELECT MIN(sequence) FROM execution_events WHERE execution_id = ?1 AND generation = ?2",
                params![id.as_str(), generation.get() as i64],
                |row| row.get(0),
            )?;
            let base_sequence = base_sequence.unwrap_or(next_sequence).max(1) as u64;
            if after_sequence.saturating_add(1) < base_sequence {
                return Ok(Some(EventPage {
                    base_sequence,
                    next_sequence: next_sequence as u64,
                    events: Vec::new(),
                }));
            }
            let mut statement = connection.prepare(
                "SELECT encoded FROM execution_events
                 WHERE execution_id = ?1 AND generation = ?2 AND sequence > ?3
                 ORDER BY sequence ASC",
            )?;
            let rows = statement.query_map(
                params![id.as_str(), generation.get() as i64, after_sequence as i64],
                |row| row.get::<_, Vec<u8>>(0),
            )?;
            let events = rows
                .map(|row| serde_json::from_slice(&row?).map_err(StoreError::from))
                .collect::<Result<Vec<ExecutionEvent>, StoreError>>()?;
            Ok(Some(EventPage {
                base_sequence,
                next_sequence: next_sequence as u64,
                events,
            }))
        })
        .await
        .map_err(|_| StoreError::Worker)?
    }

    pub async fn load_all(&self) -> Result<Vec<ExecutionSnapshot>, StoreError> {
        let connection = self.connection.clone();
        tokio::task::spawn_blocking(move || {
            let connection = connection.lock().map_err(|_| StoreError::Worker)?;
            let mut statement = connection.prepare(
                "SELECT snapshot_json FROM executions ORDER BY execution_id, generation",
            )?;
            let rows = statement.query_map([], |row| row.get::<_, Vec<u8>>(0))?;
            rows.map(|row| {
                serde_json::from_slice::<ExecutionSnapshot>(&row?).map_err(StoreError::from)
            })
            .collect()
        })
        .await
        .map_err(|_| StoreError::Worker)?
    }

    pub async fn load_snapshot(
        &self,
        id: ExecutionId,
        generation: Option<ExecutionGeneration>,
    ) -> Result<Option<ExecutionSnapshot>, StoreError> {
        let connection = self.connection.clone();
        tokio::task::spawn_blocking(move || {
            let connection = connection.lock().map_err(|_| StoreError::Worker)?;
            let row: Option<Vec<u8>> = match generation {
                Some(generation) => connection
                    .query_row(
                        "SELECT snapshot_json FROM executions WHERE execution_id = ?1 AND generation = ?2",
                        params![id.as_str(), generation.get() as i64],
                        |row| row.get(0),
                    )
                    .optional()?,
                None => connection
                    .query_row(
                        "SELECT snapshot_json FROM executions WHERE execution_id = ?1
                         ORDER BY generation DESC LIMIT 1",
                        [id.as_str()],
                        |row| row.get(0),
                    )
                    .optional()?,
            };
            row.map(|value| serde_json::from_slice(&value).map_err(StoreError::from))
                .transpose()
        })
        .await
        .map_err(|_| StoreError::Worker)?
    }

    pub async fn recover(&self) -> Result<Vec<ExecutionSnapshot>, StoreError> {
        let connection = self.connection.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = connection.lock().map_err(|_| StoreError::Worker)?;
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut statement = transaction.prepare(
                "SELECT snapshot_json FROM executions WHERE state IN ('Accepted', 'Preparing', 'Running', 'Cancelling')",
            )?;
            let rows = statement.query_map([], |row| row.get::<_, Vec<u8>>(0))?;
            let mut snapshots = rows
                .map(|row| serde_json::from_slice::<ExecutionSnapshot>(&row?).map_err(StoreError::from))
                .collect::<Result<Vec<_>, _>>()?;
            drop(statement);
            for snapshot in &mut snapshots {
                snapshot.state = ExecutionState::Interrupted;
                snapshot.result = Some(ExecutionResult {
                    state: ExecutionState::Interrupted,
                    exit_code: None,
                    failure: Some(ExecutionFailure::Interrupted),
                    stdout_bytes: 0,
                    stderr_bytes: 0,
                    stdout_omitted: 0,
                    stderr_omitted: 0,
                    cleanup_warning: None,
                    finalization_failure: None,
                    artifact_count: 0,
                });
                let generation = snapshot.generation.get() as i64;
                let sequence: i64 = transaction.query_row(
                    "SELECT next_sequence FROM executions WHERE execution_id = ?1 AND generation = ?2",
                    params![snapshot.execution_id.as_str(), generation],
                    |row| row.get(0),
                )?;
                let event = ExecutionEvent {
                    sequence: EventSequence::new(sequence as u64),
                    kind: ExecutionEventKind::State(ExecutionState::Interrupted),
                    metadata: EventMetadata { fields: vec![] },
                };
                transaction.execute(
                    "UPDATE executions SET state = 'Interrupted', next_sequence = ?3,
                       snapshot_json = ?4, updated_unix_ms = ?5
                     WHERE execution_id = ?1 AND generation = ?2",
                    params![
                        snapshot.execution_id.as_str(),
                        generation,
                        sequence + 1,
                        serde_json::to_vec(snapshot)?,
                        unix_millis(),
                    ],
                )?;
                transaction.execute(
                    "INSERT INTO execution_events(execution_id, generation, sequence, encoded)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        snapshot.execution_id.as_str(),
                        generation,
                        sequence,
                        serde_json::to_vec(&event)?,
                    ],
                )?;
                trim_events(&transaction, snapshot.execution_id.as_str(), generation)?;
            }
            transaction.commit()?;
            Ok(snapshots)
        })
        .await
        .map_err(|_| StoreError::Worker)?
    }
}

fn trim_events(
    transaction: &rusqlite::Transaction<'_>,
    id: &str,
    generation: i64,
) -> Result<(), rusqlite::Error> {
    loop {
        let (count, bytes): (i64, i64) = transaction.query_row(
            "SELECT COUNT(*), COALESCE(SUM(length(encoded)), 0) FROM execution_events
             WHERE execution_id = ?1 AND generation = ?2",
            params![id, generation],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if count as usize <= MAX_RETAINED_EVENTS && bytes as usize <= MAX_RETAINED_EVENT_BYTES {
            break;
        }
        transaction.execute(
            "DELETE FROM execution_events WHERE execution_id = ?1 AND generation = ?2
             AND sequence = (SELECT MIN(sequence) FROM execution_events
                             WHERE execution_id = ?1 AND generation = ?2)",
            params![id, generation],
        )?;
    }
    Ok(())
}

pub fn lease_hash(lease: &str) -> String {
    hex::encode(sha2::Sha256::digest(lease.as_bytes()))
}

pub fn unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

fn state_name(state: &ExecutionState) -> &'static str {
    match state {
        ExecutionState::Accepted => "Accepted",
        ExecutionState::Preparing => "Preparing",
        ExecutionState::Running => "Running",
        ExecutionState::Cancelling => "Cancelling",
        ExecutionState::Succeeded => "Succeeded",
        ExecutionState::Failed => "Failed",
        ExecutionState::Cancelled => "Cancelled",
        ExecutionState::TimedOut => "TimedOut",
        ExecutionState::Interrupted => "Interrupted",
    }
}

fn is_terminal_name(state: &str) -> bool {
    matches!(
        state,
        "Succeeded" | "Failed" | "Cancelled" | "TimedOut" | "Interrupted"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn snapshot(id: &str, generation: u64, state: ExecutionState) -> ExecutionSnapshot {
        ExecutionSnapshot {
            schema_version: 1,
            execution_id: ExecutionId::new(id).unwrap(),
            generation: ExecutionGeneration::new(generation).unwrap(),
            state,
            result: None,
        }
    }

    async fn reserve(store: &ExecutionStore, snapshot: ExecutionSnapshot) {
        assert!(matches!(
            store
                .reserve(
                    snapshot,
                    eggwork_core::CANONICAL_REQUEST_VERSION,
                    "a".repeat(64),
                    "principal-a".into(),
                    lease_hash("test-lease"),
                    unix_millis() + 60_000,
                )
                .await
                .unwrap(),
            ReserveResult::Created
        ));
    }

    #[tokio::test]
    async fn snapshot_lookup_is_principal_fenced_and_hides_foreign_execution() {
        let temp = TempDir::new().unwrap();
        let store = ExecutionStore::open(temp.path().join("state.sqlite")).unwrap();
        let accepted = snapshot("private-execution", 1, ExecutionState::Accepted);
        reserve(&store, accepted.clone()).await;
        assert_eq!(
            store
                .load_snapshot_for_principal(
                    accepted.execution_id.clone(),
                    Some(accepted.generation),
                    "principal-b".into(),
                )
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            store
                .load_snapshot_for_principal(
                    accepted.execution_id.clone(),
                    None,
                    "principal-a".into(),
                )
                .await
                .unwrap(),
            Some(accepted)
        );
    }

    #[tokio::test]
    async fn recovery_marks_uncertain_work_interrupted_and_never_replays_it() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("state.sqlite");
        let store = ExecutionStore::open(&path).unwrap();
        let accepted = snapshot("restart-live", 1, ExecutionState::Accepted);
        reserve(&store, accepted.clone()).await;
        store
            .commit_event(
                accepted.clone(),
                ExecutionEventKind::State(ExecutionState::Accepted),
            )
            .await
            .unwrap();
        let running = ExecutionSnapshot {
            state: ExecutionState::Running,
            ..accepted
        };
        store
            .commit_event(
                running.clone(),
                ExecutionEventKind::State(ExecutionState::Running),
            )
            .await
            .unwrap();
        drop(store);

        let recovered = ExecutionStore::open(&path).unwrap();
        let uncertain = recovered.recover().await.unwrap();
        assert_eq!(uncertain.len(), 1);
        assert_eq!(uncertain[0].state, ExecutionState::Interrupted);
        assert_eq!(
            uncertain[0].result.as_ref().unwrap().failure,
            Some(ExecutionFailure::Interrupted)
        );
        let page = recovered
            .load_page(
                ExecutionId::new("restart-live").unwrap(),
                ExecutionGeneration::new(1).unwrap(),
                0,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(page.events.len(), 3);
        assert_eq!(page.events[2].sequence.get(), 3);
        assert!(recovered.recover().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn event_journal_trims_by_count_and_bytes_with_explicit_base_cursor() {
        let temp = TempDir::new().unwrap();
        let store = ExecutionStore::open(temp.path().join("state.sqlite")).unwrap();
        let accepted = snapshot("event-ring", 1, ExecutionState::Accepted);
        reserve(&store, accepted.clone()).await;
        let mut current = accepted;
        for index in 0..300 {
            current.state = ExecutionState::Running;
            store
                .commit_event(
                    current.clone(),
                    ExecutionEventKind::Diagnostic(format!("{index}:{}", "x".repeat(2048))),
                )
                .await
                .unwrap();
        }
        let page = store
            .load_page(
                ExecutionId::new("event-ring").unwrap(),
                ExecutionGeneration::new(1).unwrap(),
                0,
            )
            .await
            .unwrap()
            .unwrap();
        assert!(page.base_sequence > 1);
        assert!(
            page.events.is_empty(),
            "expired cursor returns no partial replay"
        );
        let retained = store
            .load_page(
                ExecutionId::new("event-ring").unwrap(),
                ExecutionGeneration::new(1).unwrap(),
                page.base_sequence - 1,
            )
            .await
            .unwrap()
            .unwrap();
        assert!(retained.events.len() <= MAX_RETAINED_EVENTS);
        assert_eq!(
            retained.events.first().unwrap().sequence.get(),
            page.base_sequence
        );
        let encoded_bytes: usize = retained
            .events
            .iter()
            .map(|event| serde_json::to_vec(event).unwrap().len())
            .sum();
        assert!(encoded_bytes <= MAX_RETAINED_EVENT_BYTES);
    }

    #[tokio::test]
    async fn duplicate_identity_is_idempotent_and_conflicting_digest_is_rejected() {
        let temp = TempDir::new().unwrap();
        let store = ExecutionStore::open(temp.path().join("state.sqlite")).unwrap();
        let accepted = snapshot("dedupe", 1, ExecutionState::Accepted);
        reserve(&store, accepted.clone()).await;
        let duplicate = store
            .reserve(
                accepted.clone(),
                eggwork_core::CANONICAL_REQUEST_VERSION,
                "a".repeat(64),
                "principal-a".into(),
                lease_hash("test-lease"),
                unix_millis() + 60_000,
            )
            .await
            .unwrap();
        assert!(matches!(duplicate, ReserveResult::Existing(_)));
        let conflict = store
            .reserve(
                accepted,
                eggwork_core::CANONICAL_REQUEST_VERSION,
                "b".repeat(64),
                "principal-a".into(),
                lease_hash("test-lease"),
                unix_millis() + 60_000,
            )
            .await
            .unwrap();
        assert!(matches!(conflict, ReserveResult::Conflict));
    }

    #[tokio::test]
    async fn lease_renewal_replay_is_idempotent() {
        let temp = TempDir::new().unwrap();
        let store = ExecutionStore::open(temp.path().join("state.sqlite")).unwrap();
        let accepted = snapshot("lease-renew", 1, ExecutionState::Accepted);
        reserve(&store, accepted).await;
        let id = ExecutionId::new("lease-renew").unwrap();
        let generation = ExecutionGeneration::new(1).unwrap();
        let token_hash = lease_hash("test-lease");
        let requested = unix_millis() + 120_000;
        let first = store
            .update_lease(
                id.clone(),
                generation,
                token_hash.clone(),
                "renewal-1".into(),
                requested,
            )
            .await
            .unwrap()
            .unwrap();
        let replay = store
            .update_lease(
                id,
                generation,
                token_hash,
                "renewal-1".into(),
                requested + 120_000,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first, replay);
    }
}
