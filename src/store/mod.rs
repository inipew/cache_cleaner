pub mod schema;

use rusqlite::{params, Connection, OptionalExtension};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::domain::intent::{MutationType, OperationIntent};
use crate::domain::result::{OperationFinalResult, OperationStatus};
use crate::domain::types::{
    AttemptId, ByteCount, CatalogGeneration, ConfigGeneration, DeviceNumber, FileIdentity,
    InodeNumber, JobId, OperationId, PlanId, RelativePath, TargetId,
    UnixTimestamp, WorkerId,
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

        let _ = conn.busy_timeout(std::time::Duration::from_secs(5));

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

        let _ = conn.busy_timeout(std::time::Duration::from_secs(5));

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
        let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
        conn.execute_batch(
            "PRAGMA busy_timeout = 5000;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = FULL;
             PRAGMA foreign_keys = ON;",
        )
        .map_err(|e| CleanerError::Storage(format!("Failed to set SQLite pragmas: {}", e)))?;
        Ok(())
    }

    fn init_schema(conn: &Connection) -> Result<()> {
        conn.execute_batch(CREATE_TABLES_SQL)
            .map_err(|e| CleanerError::Storage(format!("Failed to initialize SQLite schema: {}", e)))?;

        // Apply column additions and migrations on existing tables
        Self::apply_migrations(conn)?;

        // Record migration
        let now = UnixTimestamp::now().as_secs();
        conn.execute(
            "INSERT OR IGNORE INTO schema_migrations (version, applied_at) VALUES (?1, ?2);",
            params![SCHEMA_VERSION, now],
        )
        .map_err(|e| CleanerError::Storage(format!("Failed to record schema migration: {}", e)))?;

        Ok(())
    }

    fn apply_migrations(conn: &Connection) -> Result<()> {
        Self::ensure_column_exists(
            conn,
            "operation_intents",
            "state",
            "TEXT NOT NULL DEFAULT 'COMMITTED'",
        )?;
        Self::ensure_column_exists(
            conn,
            "operation_intents",
            "resolved_at",
            "INTEGER",
        )?;
        Self::ensure_column_exists(
            conn,
            "jobs",
            "total_estimated_bytes",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        Self::ensure_column_exists(
            conn,
            "jobs",
            "total_reclaimed_bytes",
            "INTEGER NOT NULL DEFAULT 0",
        )?;

        // Ensure unique index on (attempt_id, op_id) for S1 aggregator invariant
        let _ = conn.execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_operation_intents_attempt_op ON operation_intents(attempt_id, op_id);",
            [],
        );

        Ok(())
    }

    fn ensure_column_exists(
        conn: &Connection,
        table: &str,
        column: &str,
        column_def: &str,
    ) -> Result<()> {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({});", table))
            .map_err(|e| CleanerError::Storage(e.to_string()))?;
        let columns: Vec<String> = stmt
            .query_map([], |row| row.get(1))
            .map_err(|e| CleanerError::Storage(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();

        if !columns.iter().any(|c| c.eq_ignore_ascii_case(column)) {
            log::info!("Migrating table {}: adding column {}", table, column);
            conn.execute(
                &format!("ALTER TABLE {} ADD COLUMN {} {};", table, column, column_def),
                [],
            )
            .map_err(|e| {
                CleanerError::Storage(format!(
                    "Failed to add column {} to {}: {}",
                    column, table, e
                ))
            })?;
        }

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
        catalog_gen: CatalogGeneration,
        config_gen: ConfigGeneration,
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
                expected_dev, expected_ino, estimated_bytes, mutation_type, state,
                catalog_generation, config_generation, committed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13);",
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
                intent.state.to_string(),
                intent.catalog_generation.0,
                intent.config_generation.0,
                intent.committed_at.as_secs(),
            ],
        )
        .map_err(|e| CleanerError::Storage(format!("Failed to commit operation intent: {}", e)))?;

        Ok(conn.last_insert_rowid())
    }

    /// Commits a batch of operation intents within a single immediate SQLite transaction.
    pub fn commit_operation_intents_batch(&self, intents: &[OperationIntent]) -> Result<()> {
        if intents.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| CleanerError::Storage(format!("Failed to begin transaction: {}", e)))?;

        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT INTO operation_intents (
                        job_id, attempt_id, op_id, target_id, rel_path, expected_dev,
                        expected_ino, estimated_bytes, mutation_type, state,
                        catalog_generation, config_generation, committed_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13);",
                )
                .map_err(|e| CleanerError::Storage(format!("Failed to prepare statement: {}", e)))?;

            for intent in intents {
                stmt.execute(params![
                    intent.job_id.0,
                    intent.attempt_id.0,
                    intent.op_id.0,
                    intent.target_id.0,
                    intent.rel_path.as_str(),
                    intent.expected_identity.dev.0,
                    intent.expected_identity.ino.0,
                    intent.estimated_bytes.as_u64(),
                    intent.mutation_type.to_string(),
                    intent.state.to_string(),
                    intent.catalog_generation.0,
                    intent.config_generation.0,
                    intent.committed_at.as_secs(),
                ])
                .map_err(|e| CleanerError::Storage(format!("Failed to insert intent in batch: {}", e)))?;
            }
        }

        tx.commit()
            .map_err(|e| CleanerError::Storage(format!("Failed to commit intent batch transaction: {}", e)))?;
        Ok(())
    }

    /// Retrieves an execution summary of a job from SQLite.
    pub fn get_job_summary(&self, job_id: JobId) -> Result<Option<(String, u64, u64)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare_cached(
                "SELECT state, total_reclaimed_bytes, updated_at FROM jobs WHERE job_id = ?1;",
            )
            .map_err(|e| CleanerError::Storage(format!("Failed to prepare query: {}", e)))?;

        let mut rows = stmt
            .query(params![job_id.0])
            .map_err(|e| CleanerError::Storage(format!("Failed to query job: {}", e)))?;

        if let Some(row) = rows.next().map_err(|e| CleanerError::Storage(e.to_string()))? {
            let state: String = row.get(0).map_err(|e| CleanerError::Storage(e.to_string()))?;
            let reclaimed: i64 = row.get(1).map_err(|e| CleanerError::Storage(e.to_string()))?;
            let updated: i64 = row.get(2).map_err(|e| CleanerError::Storage(e.to_string()))?;
            Ok(Some((state, reclaimed.max(0) as u64, updated.max(0) as u64)))
        } else {
            Ok(None)
        }
    }

    /// Updates intent state upon mutation lifecycle progression.
    pub fn update_intent_state(&self, attempt_id: AttemptId, op_id: OperationId, state: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = UnixTimestamp::now().as_secs();
        conn.execute(
            "UPDATE operation_intents SET state = ?1, resolved_at = ?2 WHERE attempt_id = ?3 AND op_id = ?4;",
            params![state, now, attempt_id.0, op_id.0],
        )
        .map_err(|e| CleanerError::Storage(format!("Failed to update intent state: {}", e)))?;
        Ok(())
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
    /// Fully compatible with SQLite 3.22+ (Android 9+) without requiring SQLite 3.35+ syntax.
    pub fn acquire_lease(&self, resource_id: &str, worker_id: WorkerId, ttl_secs: u64) -> Result<bool> {
        let mut conn = self.conn.lock().unwrap();
        let now = UnixTimestamp::now().as_secs();
        let expires_at = now + ttl_secs;
        let token = format!("lease-{}-{}", worker_id.0, now);

        let tx = conn.transaction().map_err(|e| CleanerError::Storage(e.to_string()))?;

        // 1. Delete expired lease if exists
        let _ = tx.execute(
            "DELETE FROM leases WHERE resource_id = ?1 AND expires_at <= ?2;",
            params![resource_id, now],
        );

        // 2. Check if active unexpired lease is held by another worker
        let existing: Option<(u32, u64)> = tx
            .query_row(
                "SELECT worker_id, expires_at FROM leases WHERE resource_id = ?1;",
                params![resource_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| CleanerError::Storage(e.to_string()))?;

        if let Some((active_worker, exp)) = existing {
            if active_worker != worker_id.0 && exp > now {
                return Ok(false);
            }
        }

        // 3. Insert or replace lease for this worker
        tx.execute(
            "INSERT OR REPLACE INTO leases (resource_id, worker_id, lease_token, acquired_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5);",
            params![resource_id, worker_id.0, token, now, expires_at],
        )
        .map_err(|e| CleanerError::Storage(format!("Failed to insert lease: {}", e)))?;

        tx.commit().map_err(|e| CleanerError::Storage(e.to_string()))?;
        Ok(true)
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
                        expected_dev, expected_ino, estimated_bytes, mutation_type, state,
                        catalog_generation, config_generation, committed_at, resolved_at
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
                let state_str: String = row.get(10)?;
                let cat_gen: u64 = row.get(11)?;
                let cfg_gen: u64 = row.get(12)?;
                let committed: u64 = row.get(13)?;
                let resolved: Option<u64> = row.get(14)?;

                let mutation_type = match mut_type_str.as_str() {
                    "DELETE_DIR_EMPTY" => MutationType::DeleteDirEmpty,
                    "PRUNE_DIR_RECURSIVE" => MutationType::PruneDirRecursive,
                    _ => MutationType::DeleteFile,
                };

                let state = match state_str.as_str() {
                    "MUTATING" => crate::domain::intent::IntentState::Mutating,
                    "VERIFIED_SUCCESS" => crate::domain::intent::IntentState::VerifiedSuccess,
                    "VERIFIED_FAILED" => crate::domain::intent::IntentState::VerifiedFailed,
                    "RESOLVED_UNKNOWN" => crate::domain::intent::IntentState::ResolvedUnknown,
                    _ => crate::domain::intent::IntentState::Committed,
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
                    estimated_bytes: ByteCount::new_unchecked(estimated),
                    mutation_type,
                    state,
                    catalog_generation: CatalogGeneration(cat_gen),
                    config_generation: ConfigGeneration(cfg_gen),
                    committed_at: UnixTimestamp::from_secs(committed),
                    resolved_at: resolved.map(UnixTimestamp::from_secs),
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
