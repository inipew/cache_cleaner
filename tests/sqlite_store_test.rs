use std::fs;
use std::path::PathBuf;

use cache_cleaner_daemon::domain::intent::{MutationType, OperationIntent};
use cache_cleaner_daemon::domain::result::{OperationFinalResult, OperationStatus};
use cache_cleaner_daemon::domain::types::{
    AttemptId, ByteCount, DeviceNumber, FileIdentity, GenerationId, InodeNumber, JobId,
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
        .register_job(job_id, "PERIODIC", GenerationId(1), GenerationId(1))
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
        GenerationId(1),
        GenerationId(1),
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
