use std::fs;
use std::path::PathBuf;

use cache_cleaner_daemon::domain::intent::{MutationType, OperationIntent};
use cache_cleaner_daemon::domain::types::{
    ByteCount, DeviceNumber, FileIdentity, GenerationId, InodeNumber, JobId, OperationId,
    RelativePath, TargetId,
};
use cache_cleaner_daemon::resource::ResourceManager;
use cache_cleaner_daemon::store::SqliteStore;

struct TestSandbox {
    root: PathBuf,
}

impl TestSandbox {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("cleaner_fault_test_{}_{}", name, std::process::id()));
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
fn test_target_lock_map_exclusive_isolation() {
    let mgr = ResourceManager::default();
    let target = TargetId::new("android:com.example.app:cache");

    // 1. First worker acquires lock
    let lock_guard_1 = mgr.acquire_target_lock(&target).expect("Worker 1 should acquire lock");
    assert!(mgr.is_target_locked(&target));

    // 2. Second worker attempts to lock same target -> FAILS with collision error
    let lock_attempt_2 = mgr.acquire_target_lock(&target);
    assert!(lock_attempt_2.is_err(), "Concurrent lock on same target must fail");

    // 3. Different target can still be locked concurrently
    let other_target = TargetId::new("android:com.other.app:cache");
    let other_guard = mgr.acquire_target_lock(&other_target).expect("Other target should lock");
    assert!(mgr.is_target_locked(&other_target));

    // 4. Worker 1 drops lock -> slot freed
    drop(lock_guard_1);
    assert!(!mgr.is_target_locked(&target));

    // 5. Worker 2 can now acquire lock
    let lock_guard_2 = mgr.acquire_target_lock(&target).expect("Worker 2 should now acquire lock");
    assert!(mgr.is_target_locked(&target));

    drop(other_guard);
    drop(lock_guard_2);
    assert!(!mgr.is_target_locked(&target));
    assert!(!mgr.is_target_locked(&other_target));
}

#[test]
fn test_relative_path_control_character_sanitization() {
    // Malicious control characters
    assert!(RelativePath::parse("cache/\0attack").is_none());
    assert!(RelativePath::parse("cache/\nnewline").is_none());
    assert!(RelativePath::parse("cache/\rreturn").is_none());
    assert!(RelativePath::parse("cache/\x1b[31mescape").is_none());
    assert!(RelativePath::parse("cache/\x7fdelete").is_none());

    // Valid paths
    assert!(RelativePath::parse("cache/normal_file.tmp").is_some());
    assert!(RelativePath::parse("sub_dir/another_file.log").is_some());
}

#[test]
fn test_sqlite_store_durability_and_transactions() {
    let sandbox = TestSandbox::new("sqlite_durability");
    let db_path = sandbox.root.join("cleaner.db");
    let store = SqliteStore::open_or_create(&db_path).unwrap();

    let job_id = JobId(999);
    store
        .register_job(job_id, "TEST", GenerationId::INITIAL, GenerationId::INITIAL)
        .unwrap();

    let intent = OperationIntent::new(
        job_id,
        cache_cleaner_daemon::domain::types::AttemptId(1),
        OperationId(1),
        TargetId::new("test"),
        RelativePath::parse("test.tmp").unwrap(),
        FileIdentity {
            dev: DeviceNumber(1),
            ino: InodeNumber(1),
        },
        ByteCount::new(10),
        MutationType::DeleteFile,
        GenerationId::INITIAL,
        GenerationId::INITIAL,
    );

    let row_id = store.commit_operation_intent(&intent).unwrap();
    assert!(row_id > 0);

    // Reopen store from disk and assert durability
    drop(store);
    let reopened = SqliteStore::open_or_create(&db_path).unwrap();
    let intents = reopened
        .get_operation_intents_for_attempt(cache_cleaner_daemon::domain::types::AttemptId(1))
        .unwrap();
    assert_eq!(intents.len(), 1);
    assert_eq!(intents[0].job_id, job_id);
    assert_eq!(intents[0].rel_path.as_str(), "test.tmp");
}
