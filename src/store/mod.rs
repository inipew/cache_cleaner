pub mod schema;

use rusqlite::{params, Connection};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::domain::intent::{MutationType, OperationIntent};
use crate::domain::result::{OperationFinalResult, OperationStatus};
use crate::domain::types::{
    AttemptId, ByteCount, DeviceNumber, FileIdentity, GenerationId, InodeNumber, JobId,
    OperationId, PlanId, RelativePath, TargetId, UnixTimestamp, WorkerId,
};
use crate::error::{CleanerError, Result};
use schema::{CREATE_TABLES_SQL, SCHEMA_VERSION};

/// Authoritative SQLite + WAL Durable Operation Store.
/// Enforces ACID consistency, crash boundaries, intent durability, leases, and idempotency.
#[derive(Debug, Clone)]
pub struct SqliteStore {
    db_path: PathBuf,
    conn: Arc<Mutex<Connection>>,
}

impl SqliteStore {
    pub fn open_or_create(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let conn = Connection::open(path).map_err(|e| {
            CleanerError::Storage(format!("Failed to open SQLite store at {}: {}", path.display(), e))
        })?;

        // Initialize WAL mode, pragmas, and schema
        Self::configure_pragmas(&conn)?;
        Self::init_schema(&conn)?;

        Ok(Self {
            db_path: path.to_path_buf(),
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(|e| {
            CleanerError::Storage(format!("Failed to open in-memory SQLite store: {}", e))
        })?;

        Self::configure_pragmas(&conn)?;
        Self::init_schema(&conn)?;

        Ok(Self {
            db_path: PathBuf::from(":memory:"),
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn default_store() -> Result<Self> {
        let primary = Path::new("/data/adb/cleaner/store/cleaner.db");
        if primary.parent().is_some_and(|p| p.exists()) {
            Self::open_or_create(primary)
        } else {
            let local_dir = std::env::temp_dir().join("cleaner_store");
            let _ = fs::create_dir_all(&local_dir);
            Self::open_or_create(&local_dir.join("cleaner.db"))
        }
    }

    fn configure_pragmas(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )
        .map_err(|e| CleanerError::Storage(format!("Failed to set SQLite pragmas: {}", e)))?;
        Ok(())
    }

    fn init_schema(conn: &Connection) -> Result<()> {
        conn.execute_batch(CREATE_TABLES_SQL)
            .map_err(|e| CleanerError::Storage(format!("Failed to initialize SQLite schema: {}", e)))?;

        // Record migration
        let now = UnixTimestamp::now().as_secs();
        conn.execute(
            "INSERT OR IGNORE INTO schema_migrations (version, applied_at) VALUES (?1, ?2);",
            params![SCHEMA_VERSION, now],
        )
        .map_err(|e| CleanerError::Storage(format!("Failed to record schema migration: {}", e)))?;

        Ok(())
    }

    /// Returns maximum existing job_id in SQLite store.
    pub fn get_max_job_id(&self) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        let max_id: Option<u64> = conn
            .query_row("SELECT MAX(job_id) FROM jobs;", [], |row| row.get(0))
            .unwrap_or(None);
        Ok(max_id.unwrap_or(0))
    }

    /// Registers a new admitted job in the store.
    pub fn register_job(
        &self,
        job_id: JobId,
        job_type: &str,
        catalog_gen: GenerationId,
        config_gen: GenerationId,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = UnixTimestamp::now().as_secs();
        conn.execute(
            "INSERT INTO jobs (job_id, job_type, state, catalog_generation, config_generation, created_at, updated_at)
             VALUES (?1, ?2, 'ADMITTED', ?3, ?4, ?5, ?5)
             ON CONFLICT(job_id) DO UPDATE SET updated_at = ?5;",
            params![job_id.0, job_type, catalog_gen.0, config_gen.0, now],
        )
        .map_err(|e| CleanerError::Storage(format!("Failed to register job {}: {}", job_id, e)))?;
        Ok(())
    }

    /// Updates state and reclaimed bytes for a job.
    pub fn update_job_state(
        &self,
        job_id: JobId,
        state: &str,
        total_reclaimed: ByteCount,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = UnixTimestamp::now().as_secs();
        conn.execute(
            "UPDATE jobs SET state = ?1, total_reclaimed_bytes = ?2, updated_at = ?3 WHERE job_id = ?4;",
            params![state, total_reclaimed.as_u64(), now, job_id.0],
        )
        .map_err(|e| CleanerError::Storage(format!("Failed to update job {} state: {}", job_id, e)))?;
        Ok(())
    }

    /// Returns maximum existing attempt_id in SQLite store.
    pub fn get_max_attempt_id(&self) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        let max_id: Option<u64> = conn
            .query_row("SELECT MAX(attempt_id) FROM attempts;", [], |row| row.get(0))
            .unwrap_or(None);
        Ok(max_id.unwrap_or(0))
    }

    /// Claims an execution attempt for a job.
    pub fn create_attempt(
        &self,
        attempt_id: AttemptId,
        job_id: JobId,
        worker_id: WorkerId,
        lease_duration_secs: u64,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = UnixTimestamp::now().as_secs();
        let lease_expires = now + lease_duration_secs;
        conn.execute(
            "INSERT INTO attempts (attempt_id, job_id, worker_id, state, lease_expires_at, started_at)
             VALUES (?1, ?2, ?3, 'RUNNING', ?4, ?5)
             ON CONFLICT(attempt_id) DO UPDATE SET state = 'RUNNING', lease_expires_at = ?4;",
            params![attempt_id.0, job_id.0, worker_id.0, lease_expires, now],
        )
        .map_err(|e| CleanerError::Storage(format!("Failed to create attempt {}: {}", attempt_id, e)))?;
        Ok(())
    }

    /// Updates attempt state upon completion or failure.
    pub fn update_attempt_state(&self, attempt_id: AttemptId, state: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = UnixTimestamp::now().as_secs();
        conn.execute(
            "UPDATE attempts SET state = ?1, finished_at = ?2 WHERE attempt_id = ?3;",
            params![state, now, attempt_id.0],
        )
        .map_err(|e| CleanerError::Storage(format!("Failed to update attempt {} state: {}", attempt_id, e)))?;
        Ok(())
    }

    /// Durably commits an OperationIntent BEFORE physical filesystem mutation.
    /// This is a HARD INVARIANT: If this commit fails, physical mutation MUST NOT proceed.
    pub fn commit_operation_intent(&self, intent: &OperationIntent) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO operation_intents (
                job_id, attempt_id, op_id, target_id, rel_path,
                expected_dev, expected_ino, estimated_bytes, mutation_type,
                catalog_generation, config_generation, committed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12);",
            params![
                intent.job_id.0,
                intent.attempt_id.0,
                intent.op_id.0,
                intent.target_id.0,
                intent.rel_path.as_str(),
                intent.expected_identity.dev.0,
                intent.expected_identity.ino.0,
                intent.estimated_bytes.as_u64(),
                intent.mutation_type.to_string(),
                intent.catalog_generation.0,
                intent.config_generation.0,
                intent.committed_at.as_secs(),
            ],
        )
        .map_err(|e| CleanerError::Storage(format!("Failed to commit operation intent: {}", e)))?;

        Ok(conn.last_insert_rowid())
    }

    /// Records the finalized result of an operation in the database.
    #[allow(clippy::too_many_arguments)]
    pub fn record_operation_result(
        &self,
        job_id: JobId,
        plan_id: PlanId,
        target_id: &TargetId,
        op_type: &str,
        rel_path: &str,
        expected_id: &FileIdentity,
        estimated_bytes: ByteCount,
        result: &OperationFinalResult,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let (status_str, error_msg) = match &result.status {
            OperationStatus::Success => ("SUCCESS", None),
            OperationStatus::Failed { error } => ("FAILED", Some(error.clone())),
            OperationStatus::VerificationFailed { reason } => ("VERIFICATION_FAILED", Some(reason.clone())),
            OperationStatus::Preempted => ("PREEMPTED", None),
            OperationStatus::Skipped { reason } => ("SKIPPED", Some(reason.clone())),
        };

        conn.execute(
            "INSERT OR REPLACE INTO operations (
                job_id, op_id, plan_id, target_id, op_type, rel_path,
                expected_dev, expected_ino, estimated_bytes, status,
                reclaimed_bytes, error_message, executed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13);",
            params![
                job_id.0,
                result.op_id.0,
                plan_id.0,
                target_id.0,
                op_type,
                rel_path,
                expected_id.dev.0,
                expected_id.ino.0,
                estimated_bytes.as_u64(),
                status_str,
                result.reclaimed_bytes.as_u64(),
                error_msg,
                result.executed_at.as_secs(),
            ],
        )
        .map_err(|e| CleanerError::Storage(format!("Failed to record operation result: {}", e)))?;

        Ok(())
    }

    /// Acquires a resource lease. Fails if lease is currently active and unexpired.
    pub fn acquire_lease(&self, resource_id: &str, worker_id: WorkerId, ttl_secs: u64) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let now = UnixTimestamp::now().as_secs();
        let expires_at = now + ttl_secs;
        let token = format!("lease-{}-{}", worker_id.0, now);

        // Delete expired lease if exists
        let _ = conn.execute(
            "DELETE FROM leases WHERE resource_id = ?1 AND expires_at <= ?2;",
            params![resource_id, now],
        );

        let res = conn.execute(
            "INSERT INTO leases (resource_id, worker_id, lease_token, acquired_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(resource_id) DO UPDATE SET
                worker_id = ?2,
                lease_token = ?3,
                acquired_at = ?4,
                expires_at = ?5
             WHERE leases.expires_at <= ?4 OR leases.worker_id = ?2;",
            params![resource_id, worker_id.0, token, now, expires_at],
        );

        match res {
            Ok(count) => Ok(count > 0),
            Err(_) => Ok(false),
        }
    }

    /// Releases a resource lease.
    pub fn release_lease(&self, resource_id: &str, worker_id: WorkerId) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM leases WHERE resource_id = ?1 AND worker_id = ?2;",
            params![resource_id, worker_id.0],
        )
        .map_err(|e| CleanerError::Storage(format!("Failed to release lease {}: {}", resource_id, e)))?;
        Ok(())
    }

    /// Checks if an idempotency key exists. If not, reserves it.
    pub fn check_or_insert_idempotency(&self, key: &str, job_id: JobId, ttl_secs: u64) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let now = UnixTimestamp::now().as_secs();

        // 1. Check existing
        let mut stmt = conn
            .prepare("SELECT response_payload FROM idempotency_keys WHERE idempotency_key = ?1 AND expires_at > ?2;")
            .map_err(|e| CleanerError::Storage(e.to_string()))?;

        let mut rows = stmt
            .query(params![key, now])
            .map_err(|e| CleanerError::Storage(e.to_string()))?;

        if let Some(row) = rows.next().map_err(|e| CleanerError::Storage(e.to_string()))? {
            let payload: Option<String> = row.get(0).map_err(|e| CleanerError::Storage(e.to_string()))?;
            return Ok(payload);
        }

        // 2. Insert new reservation
        let expires_at = now + ttl_secs;
        conn.execute(
            "INSERT OR REPLACE INTO idempotency_keys (idempotency_key, job_id, response_payload, created_at, expires_at)
             VALUES (?1, ?2, NULL, ?3, ?4);",
            params![key, job_id.0, now, expires_at],
        )
        .map_err(|e| CleanerError::Storage(format!("Failed to insert idempotency key: {}", e)))?;

        Ok(None)
    }

    /// Sets the stored response payload for an idempotency key.
    pub fn set_idempotency_response(&self, key: &str, response_payload: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE idempotency_keys SET response_payload = ?1 WHERE idempotency_key = ?2;",
            params![response_payload, key],
        )
        .map_err(|e| CleanerError::Storage(format!("Failed to set idempotency response: {}", e)))?;
        Ok(())
    }

    /// Appends an event to the transactional outbox table.
    pub fn append_outbox(&self, event_type: &str, payload: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = UnixTimestamp::now().as_secs();
        conn.execute(
            "INSERT INTO outbox (event_type, payload, status, created_at) VALUES (?1, ?2, 'PENDING', ?3);",
            params![event_type, payload, now],
        )
        .map_err(|e| CleanerError::Storage(format!("Failed to append outbox event: {}", e)))?;
        Ok(())
    }

    /// Retrieves uncompleted execution attempts (for startup crash recovery).
    pub fn get_uncompleted_attempts(&self) -> Result<Vec<(AttemptId, JobId)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT attempt_id, job_id FROM attempts WHERE state = 'RUNNING';")
            .map_err(|e| CleanerError::Storage(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| {
                let attempt_id: u64 = row.get(0)?;
                let job_id: u64 = row.get(1)?;
                Ok((AttemptId(attempt_id), JobId(job_id)))
            })
            .map_err(|e| CleanerError::Storage(e.to_string()))?;

        let mut list = Vec::new();
        for r in rows {
            list.push(r.map_err(|e| CleanerError::Storage(e.to_string()))?);
        }
        Ok(list)
    }

    /// Retrieves all committed operation intents for a specific attempt.
    pub fn get_operation_intents_for_attempt(&self, attempt_id: AttemptId) -> Result<Vec<OperationIntent>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT intent_id, job_id, attempt_id, op_id, target_id, rel_path,
                        expected_dev, expected_ino, estimated_bytes, mutation_type,
                        catalog_generation, config_generation, committed_at
                 FROM operation_intents WHERE attempt_id = ?1 ORDER BY intent_id ASC;",
            )
            .map_err(|e| CleanerError::Storage(e.to_string()))?;

        let rows = stmt
            .query_map(params![attempt_id.0], |row| {
                let intent_id: i64 = row.get(0)?;
                let job_id: u64 = row.get(1)?;
                let attempt_id: u64 = row.get(2)?;
                let op_id: u64 = row.get(3)?;
                let target_id_str: String = row.get(4)?;
                let rel_path_str: String = row.get(5)?;
                let dev: u64 = row.get(6)?;
                let ino: u64 = row.get(7)?;
                let estimated: u64 = row.get(8)?;
                let mut_type_str: String = row.get(9)?;
                let cat_gen: u64 = row.get(10)?;
                let cfg_gen: u64 = row.get(11)?;
                let committed: u64 = row.get(12)?;

                let mutation_type = match mut_type_str.as_str() {
                    "DELETE_DIR_EMPTY" => MutationType::DeleteDirEmpty,
                    "PRUNE_DIR_RECURSIVE" => MutationType::PruneDirRecursive,
                    _ => MutationType::DeleteFile,
                };

                Ok(OperationIntent {
                    intent_id: Some(intent_id),
                    job_id: JobId(job_id),
                    attempt_id: AttemptId(attempt_id),
                    op_id: OperationId(op_id),
                    target_id: TargetId::new(target_id_str),
                    rel_path: RelativePath::parse(&rel_path_str).unwrap_or_else(RelativePath::empty),
                    expected_identity: FileIdentity {
                        dev: DeviceNumber(dev),
                        ino: InodeNumber(ino),
                    },
                    estimated_bytes: ByteCount::new(estimated),
                    mutation_type,
                    catalog_generation: GenerationId(cat_gen),
                    config_generation: GenerationId(cfg_gen),
                    committed_at: UnixTimestamp::from_secs(committed),
                })
            })
            .map_err(|e| CleanerError::Storage(e.to_string()))?;

        let mut list = Vec::new();
        for r in rows {
            list.push(r.map_err(|e| CleanerError::Storage(e.to_string()))?);
        }
        Ok(list)
    }
}
