pub const SCHEMA_VERSION: u32 = 3;

pub const CREATE_TABLES_SQL: &str = r#"
-- Schema migrations table
CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at INTEGER NOT NULL
);

-- Authoritative Jobs table
CREATE TABLE IF NOT EXISTS jobs (
    job_id INTEGER PRIMARY KEY,
    job_type TEXT NOT NULL,
    state TEXT NOT NULL,
    catalog_generation INTEGER NOT NULL,
    config_generation INTEGER NOT NULL,
    total_estimated_bytes INTEGER NOT NULL DEFAULT 0,
    total_reclaimed_bytes INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- Attempts tracking table
CREATE TABLE IF NOT EXISTS attempts (
    attempt_id INTEGER PRIMARY KEY,
    job_id INTEGER NOT NULL,
    worker_id INTEGER NOT NULL,
    state TEXT NOT NULL,
    lease_expires_at INTEGER NOT NULL,
    started_at INTEGER NOT NULL,
    finished_at INTEGER,
    FOREIGN KEY(job_id) REFERENCES jobs(job_id) ON DELETE CASCADE
);

-- Durable Operation Intents (must be committed BEFORE mutation)
CREATE TABLE IF NOT EXISTS operation_intents (
    intent_id INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id INTEGER NOT NULL,
    attempt_id INTEGER NOT NULL,
    op_id INTEGER NOT NULL,
    target_id TEXT NOT NULL,
    rel_path TEXT NOT NULL,
    expected_dev INTEGER NOT NULL,
    expected_ino INTEGER NOT NULL,
    estimated_bytes INTEGER NOT NULL,
    mutation_type TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'COMMITTED',
    catalog_generation INTEGER NOT NULL,
    config_generation INTEGER NOT NULL,
    committed_at INTEGER NOT NULL,
    resolved_at INTEGER,
    UNIQUE(attempt_id, op_id),
    FOREIGN KEY(job_id) REFERENCES jobs(job_id) ON DELETE CASCADE
);

-- Authoritative Operations table
CREATE TABLE IF NOT EXISTS operations (
    job_id INTEGER NOT NULL,
    op_id INTEGER NOT NULL,
    plan_id INTEGER NOT NULL,
    target_id TEXT NOT NULL,
    op_type TEXT NOT NULL,
    rel_path TEXT NOT NULL,
    expected_dev INTEGER NOT NULL,
    expected_ino INTEGER NOT NULL,
    estimated_bytes INTEGER NOT NULL,
    status TEXT NOT NULL,
    reclaimed_bytes INTEGER NOT NULL DEFAULT 0,
    error_message TEXT,
    executed_at INTEGER NOT NULL,
    PRIMARY KEY(job_id, op_id),
    FOREIGN KEY(job_id) REFERENCES jobs(job_id) ON DELETE CASCADE
);

-- Mutual Exclusion Leases table
CREATE TABLE IF NOT EXISTS leases (
    resource_id TEXT PRIMARY KEY,
    worker_id INTEGER NOT NULL,
    lease_token TEXT NOT NULL,
    acquired_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);

-- Idempotency Keys table
CREATE TABLE IF NOT EXISTS idempotency_keys (
    idempotency_key TEXT PRIMARY KEY,
    job_id INTEGER NOT NULL,
    response_payload TEXT,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);

-- Outbox Events table for reliable asynchronous notification publication
CREATE TABLE IF NOT EXISTS outbox (
    event_id INTEGER PRIMARY KEY AUTOINCREMENT,
    event_type TEXT NOT NULL,
    payload TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'PENDING',
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_attempts_job_id ON attempts(job_id);
CREATE INDEX IF NOT EXISTS idx_operation_intents_job_attempt ON operation_intents(job_id, attempt_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_operation_intents_attempt_op ON operation_intents(attempt_id, op_id);
CREATE INDEX IF NOT EXISTS idx_operations_job_id ON operations(job_id);
CREATE INDEX IF NOT EXISTS idx_leases_expires_at ON leases(expires_at);
"#;
