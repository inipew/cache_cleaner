use std::fs;
use std::path::PathBuf;

use cache_cleaner_daemon::domain::intent::{MutationType, OperationIntent};
use cache_cleaner_daemon::domain::result::{OperationFinalResult, OperationStatus};
use cache_cleaner_daemon::domain::types::{
    AttemptId, ByteCount, DeviceNumber, FileIdentity, CatalogGeneration, ConfigGeneration, InodeNumber, JobId,
    OperationId, PlanId, RelativePath, TargetId, UnixTimestamp, WorkerId,
};
use cache_cleaner_daemon::store::SqliteStore;

struct TestSandbox {
    root: PathBuf,
}

impl TestSandbox {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("cleaner_sqlite_test_{}_{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("Failed to create test sandbox dir");
        Self { root }
    }
}

impl Drop for TestSandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn test_sqlite_store_lifecycle_and_pragmas() {
    let sandbox = TestSandbox::new("lifecycle");
    let db_path = sandbox.root.join("test.db");
    let store = SqliteStore::open_or_create(&db_path).expect("Store creation");

    // 1. Register Job
    let job_id = JobId(101);
    store
        .register_job(job_id, "PERIODIC", CatalogGeneration(1), ConfigGeneration(1))
        .expect("Register job");

    // 2. Claim Attempt with Lease
    let attempt_id = AttemptId(1);
    let worker_id = WorkerId(42);
    store
        .create_attempt(attempt_id, job_id, worker_id, 60)
        .expect("Create attempt");

    // 3. Commit Durable Operation Intent
    let intent = OperationIntent::new(
        job_id,
        attempt_id,
        OperationId(1),
        TargetId::new("android:com.example:cache"),
        RelativePath::parse("cache/data.tmp").unwrap(),
        FileIdentity {
            dev: DeviceNumber(1),
            ino: InodeNumber(100),
        },
        ByteCount::new(4096),
        MutationType::DeleteFile,
        CatalogGeneration(1),
        ConfigGeneration(1),
    );
    let row_id = store.commit_operation_intent(&intent).expect("Commit intent");
    assert!(row_id > 0);

    // 4. Record Operation Result
    let res = OperationFinalResult {
        op_id: OperationId(1),
        status: OperationStatus::Success,
        reclaimed_bytes: ByteCount::new(4096),
        executed_at: UnixTimestamp::now(),
    };
    store
        .record_operation_result(
            job_id,
            PlanId(1),
            &intent.target_id,
            "DELETE_FILE",
            intent.rel_path.as_str(),
            &intent.expected_identity,
            intent.estimated_bytes,
            &res,
        )
        .expect("Record result");

    // 5. Update Attempt & Job State
    store.update_attempt_state(attempt_id, "SUCCESS").expect("Update attempt");
    store
        .update_job_state(job_id, "COMPLETED", ByteCount::new(4096))
        .expect("Update job state");
}

#[test]
fn test_sqlite_leases_and_idempotency() {
    let store = SqliteStore::in_memory().expect("In-memory store");
    let worker_1 = WorkerId(1);
    let worker_2 = WorkerId(2);

    // 1. Acquire Lease
    let acquired = store.acquire_lease("package:com.example", worker_1, 60).unwrap();
    assert!(acquired);

    // 2. Concurrent Acquisition Fails
    let second = store.acquire_lease("package:com.example", worker_2, 60).unwrap();
    assert!(!second, "Concurrent worker must not acquire same active lease");

    // 3. Release Lease
    store.release_lease("package:com.example", worker_1).unwrap();
    let retry = store.acquire_lease("package:com.example", worker_2, 60).unwrap();
    assert!(retry, "Worker 2 can acquire after worker 1 releases");

    // 4. Idempotency Key
    let key = "clean-req-12345";
    let first_check = store.check_or_insert_idempotency(key, JobId(1), 60).unwrap();
    assert!(first_check.is_none(), "First check should reserve key");

    store.set_idempotency_response(key, r#"{"status":"OK","freed":1024}"#).unwrap();
    let second_check = store.check_or_insert_idempotency(key, JobId(1), 60).unwrap();
    assert_eq!(second_check, Some(r#"{"status":"OK","freed":1024}"#.to_string()));
}

#[test]
fn test_sqlite_batch_intents_and_job_summary() {
    let store = SqliteStore::in_memory().expect("In-memory store");
    let job_id = JobId(500);
    let attempt_id = AttemptId(1);
    store
        .register_job(job_id, "MANUAL", CatalogGeneration(1), ConfigGeneration(1))
        .expect("Register job");
    store
        .create_attempt(attempt_id, job_id, WorkerId(10), 60)
        .expect("Create attempt");

    let intents = vec![
        OperationIntent::new(
            job_id,
            attempt_id,
            OperationId(1),
            TargetId::new("target:1"),
            RelativePath::parse("file_1.tmp").unwrap(),
            FileIdentity::new(1, 10),
            ByteCount::new(100),
            MutationType::DeleteFile,
            CatalogGeneration(1),
            ConfigGeneration(1),
        ),
        OperationIntent::new(
            job_id,
            attempt_id,
            OperationId(2),
            TargetId::new("target:1"),
            RelativePath::parse("file_2.tmp").unwrap(),
            FileIdentity::new(1, 11),
            ByteCount::new(200),
            MutationType::DeleteFile,
            CatalogGeneration(1),
            ConfigGeneration(1),
        ),
    ];

    // Batch insert must succeed under atomic transaction
    store.commit_operation_intents_batch(&intents).expect("Batch intent commit");

    // Update job state and verify get_job_summary
    store.update_job_state(job_id, "COMPLETED", ByteCount::new(300)).expect("Update job state");
    let summary = store.get_job_summary(job_id).expect("Query summary");
    assert!(summary.is_some());
    let (state, reclaimed, _) = summary.unwrap();
    assert_eq!(state, "COMPLETED");
    assert_eq!(reclaimed, 300);
}

#[test]
fn test_sqlite_schema_migration_from_v1_legacy() {
    let sandbox = TestSandbox::new("migration");
    let db_path = sandbox.root.join("legacy_v1.db");

    // 1. Manually create a legacy v1 database WITHOUT 'state' and 'resolved_at' in operation_intents
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL);
            INSERT INTO schema_migrations (version, applied_at) VALUES (1, 1000);

            CREATE TABLE jobs (
                job_id INTEGER PRIMARY KEY, job_type TEXT NOT NULL, state TEXT NOT NULL,
                catalog_generation INTEGER NOT NULL, config_generation INTEGER NOT NULL,
                total_estimated_bytes INTEGER NOT NULL DEFAULT 0, total_reclaimed_bytes INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
            );

            CREATE TABLE attempts (
                attempt_id INTEGER PRIMARY KEY, job_id INTEGER NOT NULL, worker_id INTEGER NOT NULL,
                state TEXT NOT NULL, lease_expires_at INTEGER NOT NULL, started_at INTEGER NOT NULL,
                finished_at INTEGER
            );

            -- Legacy operation_intents without state and resolved_at
            CREATE TABLE operation_intents (
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
                catalog_generation INTEGER NOT NULL,
                config_generation INTEGER NOT NULL,
                committed_at INTEGER NOT NULL
            );
            "#,
        )
        .unwrap();
    }

    // 2. Open with SqliteStore - should automatically migrate schema by adding missing columns
    let store = SqliteStore::open_or_create(&db_path).expect("Store must successfully migrate legacy DB");

    // 3. Register a job and attempt
    let job_id = JobId(777);
    let attempt_id = AttemptId(1);
    store
        .register_job(job_id, "PERIODIC", CatalogGeneration(1), ConfigGeneration(1))
        .expect("Register job");
    store
        .create_attempt(attempt_id, job_id, WorkerId(1), 60)
        .expect("Create attempt");

    let intents = vec![OperationIntent::new(
        job_id,
        attempt_id,
        OperationId(1),
        TargetId::new("pkg:cache"),
        RelativePath::parse("migrated.tmp").unwrap(),
        FileIdentity::new(1, 555),
        ByteCount::new(1024),
        MutationType::DeleteFile,
        CatalogGeneration(1),
        ConfigGeneration(1),
    )];

    // 4. Batch commit MUST NOT fail with "no column named state"
    store
        .commit_operation_intents_batch(&intents)
        .expect("Batch commit must succeed on migrated table");

    // 5. Query back to verify state was written
    let queried = store
        .get_operation_intents_for_attempt(attempt_id)
        .expect("Query intents");
    assert_eq!(queried.len(), 1);
    assert_eq!(
        queried[0].state,
        cache_cleaner_daemon::domain::intent::IntentState::Committed
    );
}
