//! Durable local synchronization operations and stable frontend contracts.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SyncCommand {
    Connect { ticket: String },
    Refresh,
    Retry { operation_id: i64 },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Connectivity {
    Offline,
    Connecting,
    Direct,
    Relay,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum OperationStatus {
    Pending,
    InFlight,
    Failed,
    Complete,
}

impl OperationStatus {
    fn parse(value: &str) -> Self {
        match value {
            "in_flight" => Self::InFlight,
            "failed" => Self::Failed,
            "complete" => Self::Complete,
            _ => Self::Pending,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DurableOperation {
    pub id: i64,
    pub kind: String,
    pub target: String,
    pub status: OperationStatus,
    pub attempts: u32,
    pub last_error: Option<String>,
    pub created_ms: u64,
    pub attempted_ms: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SyncStatus {
    pub connectivity: Connectivity,
    pub converged: bool,
    pub pending_operations: usize,
    pub missing_blobs: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SyncEvent {
    StatusChanged(SyncStatus),
    OperationChanged(DurableOperation),
    MissingBlob { hash: String },
    BlobAvailable { hash: String },
}

#[derive(Debug, Error)]
pub enum SyncStateError {
    #[error("sync-state SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("sync-state lock was poisoned")]
    Poisoned,
    #[error("sync operation does not exist: {0}")]
    MissingOperation(i64),
}

#[derive(Debug)]
pub struct SyncStateStore {
    connection: Mutex<Connection>,
}

impl SyncStateStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SyncStateError> {
        let connection = Connection::open(path)?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS sync_operations (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 kind TEXT NOT NULL,
                 target TEXT NOT NULL,
                 status TEXT NOT NULL,
                 attempts INTEGER NOT NULL DEFAULT 0,
                 last_error TEXT,
                 created_ms INTEGER NOT NULL,
                 attempted_ms INTEGER
             );
             CREATE TABLE IF NOT EXISTS missing_blobs (
                 hash TEXT PRIMARY KEY
             );
             CREATE TABLE IF NOT EXISTS sync_meta (
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );",
        )?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn enqueue(
        &self,
        kind: &str,
        target: &str,
        created_ms: u64,
    ) -> Result<DurableOperation, SyncStateError> {
        let connection = self.connection()?;
        connection.execute(
            "INSERT INTO sync_operations(kind, target, status, created_ms)
             VALUES (?1, ?2, 'pending', ?3)",
            params![kind, target, sqlite_ms(created_ms)],
        )?;
        Self::get_locked(&connection, connection.last_insert_rowid())
    }

    pub fn ready(&self) -> Result<Vec<DurableOperation>, SyncStateError> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT id, kind, target, status, attempts, last_error, created_ms, attempted_ms
             FROM sync_operations WHERE status IN ('pending', 'failed') ORDER BY id",
        )?;
        let rows = statement.query_map([], operation_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn mark_attempt(
        &self,
        id: i64,
        attempted_ms: u64,
    ) -> Result<DurableOperation, SyncStateError> {
        self.update(
            id,
            "UPDATE sync_operations SET status='in_flight', attempts=attempts+1,
             last_error=NULL, attempted_ms=?1 WHERE id=?2",
            params![sqlite_ms(attempted_ms), id],
        )
    }

    pub fn mark_failed(&self, id: i64, error: &str) -> Result<DurableOperation, SyncStateError> {
        self.update(
            id,
            "UPDATE sync_operations SET status='failed', last_error=?1 WHERE id=?2",
            params![error, id],
        )
    }

    pub fn mark_complete(&self, id: i64) -> Result<DurableOperation, SyncStateError> {
        self.update(
            id,
            "UPDATE sync_operations SET status='complete', last_error=NULL WHERE id=?1",
            params![id],
        )
    }

    pub fn retry(&self, id: i64) -> Result<DurableOperation, SyncStateError> {
        self.update(
            id,
            "UPDATE sync_operations SET status='pending', last_error=NULL WHERE id=?1",
            params![id],
        )
    }

    pub fn set_connectivity(&self, connectivity: &Connectivity) -> Result<(), SyncStateError> {
        let connection = self.connection()?;
        connection.execute(
            "INSERT INTO sync_meta(key, value) VALUES ('connectivity', ?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [connectivity_name(connectivity)],
        )?;
        Ok(())
    }

    pub fn record_missing_blob(&self, hash: &str) -> Result<(), SyncStateError> {
        self.connection()?.execute(
            "INSERT OR IGNORE INTO missing_blobs(hash) VALUES (?1)",
            [hash],
        )?;
        Ok(())
    }

    pub fn resolve_blob(&self, hash: &str) -> Result<(), SyncStateError> {
        self.connection()?
            .execute("DELETE FROM missing_blobs WHERE hash=?1", [hash])?;
        Ok(())
    }

    pub fn status(&self) -> Result<SyncStatus, SyncStateError> {
        let connection = self.connection()?;
        let connectivity = connection
            .query_row(
                "SELECT value FROM sync_meta WHERE key='connectivity'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map_or(Connectivity::Offline, |value| parse_connectivity(&value));
        let pending_operations: i64 = connection.query_row(
            "SELECT COUNT(*) FROM sync_operations WHERE status != 'complete'",
            [],
            |row| row.get(0),
        )?;
        let mut statement = connection.prepare("SELECT hash FROM missing_blobs ORDER BY hash")?;
        let missing_blobs = statement
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(SyncStatus {
            converged: pending_operations == 0 && missing_blobs.is_empty(),
            connectivity,
            pending_operations: usize::try_from(pending_operations).unwrap_or(usize::MAX),
            missing_blobs,
        })
    }

    fn update<P: rusqlite::Params>(
        &self,
        id: i64,
        sql: &str,
        params: P,
    ) -> Result<DurableOperation, SyncStateError> {
        let connection = self.connection()?;
        if connection.execute(sql, params)? == 0 {
            return Err(SyncStateError::MissingOperation(id));
        }
        Self::get_locked(&connection, id)
    }

    fn get_locked(connection: &Connection, id: i64) -> Result<DurableOperation, SyncStateError> {
        connection
            .query_row(
                "SELECT id, kind, target, status, attempts, last_error, created_ms, attempted_ms
                 FROM sync_operations WHERE id=?1",
                [id],
                operation_from_row,
            )
            .optional()?
            .ok_or(SyncStateError::MissingOperation(id))
    }

    fn connection(&self) -> Result<MutexGuard<'_, Connection>, SyncStateError> {
        self.connection.lock().map_err(|_| SyncStateError::Poisoned)
    }
}

fn operation_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DurableOperation> {
    let status: String = row.get(3)?;
    Ok(DurableOperation {
        id: row.get(0)?,
        kind: row.get(1)?,
        target: row.get(2)?,
        status: OperationStatus::parse(&status),
        attempts: row.get(4)?,
        last_error: row.get(5)?,
        created_ms: u64::try_from(row.get::<_, i64>(6)?).unwrap_or_default(),
        attempted_ms: row
            .get::<_, Option<i64>>(7)?
            .and_then(|value| u64::try_from(value).ok()),
    })
}

fn connectivity_name(value: &Connectivity) -> &'static str {
    match value {
        Connectivity::Offline => "offline",
        Connectivity::Connecting => "connecting",
        Connectivity::Direct => "direct",
        Connectivity::Relay => "relay",
    }
}

fn sqlite_ms(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn parse_connectivity(value: &str) -> Connectivity {
    match value {
        "connecting" => Connectivity::Connecting,
        "direct" => Connectivity::Direct,
        "relay" => Connectivity::Relay,
        _ => Connectivity::Offline,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operations_retries_and_missing_blobs_survive_restart() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sync.sqlite");
        let store = SyncStateStore::open(&path).unwrap();
        let operation = store.enqueue("publish-head", "note002", 10).unwrap();
        store.mark_attempt(operation.id, 20).unwrap();
        store.mark_failed(operation.id, "offline").unwrap();
        store.record_missing_blob("abc").unwrap();
        store.set_connectivity(&Connectivity::Offline).unwrap();
        drop(store);

        let restored = SyncStateStore::open(path).unwrap();
        assert_eq!(restored.ready().unwrap()[0].attempts, 1);
        assert_eq!(restored.status().unwrap().missing_blobs, vec!["abc"]);
        restored.retry(operation.id).unwrap();
        restored.resolve_blob("abc").unwrap();
        restored.mark_complete(operation.id).unwrap();
        assert!(restored.status().unwrap().converged);
    }
}
