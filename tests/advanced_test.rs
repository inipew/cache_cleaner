use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;

use cache_cleaner_daemon::audit::AuditLogger;
use cache_cleaner_daemon::catalog::TargetCatalog;
use cache_cleaner_daemon::domain::intent::{MutationType, OperationIntent};
use cache_cleaner_daemon::domain::result::JobResult;
use cache_cleaner_daemon::domain::types::{
    AttemptId, ByteCount, DeviceNumber, FileIdentity, GenerationId, InodeNumber, JobId, OperationId,
    RelativePath, TargetId, WorkerId,
};
use cache_cleaner_daemon::recovery::RecoveryEngine;
use cache_cleaner_daemon::resource::ResourceManager;
use cache_cleaner_daemon::scanner::CandidateScanner;
use cache_cleaner_daemon::store::SqliteStore;
use cache_cleaner_daemon::verifier::PostconditionVerifier;

struct TestSandbox {
    root: PathBuf,
}

impl TestSandbox {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("cleaner_adv_test_{}_{}", name, std::process::id()));
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
fn test_sqlite_and_crash_recovery_reconciliation() {
    let sandbox = TestSandbox::new("sqlite_recovery");
    let db_path = sandbox.root.join("test_recovery.db");
    let store = SqliteStore::open_or_create(&db_path).unwrap();

    // 1. Setup simulated job on disk
    let cache_dir = sandbox.root.join("testapp").join("cache");
    fs::create_dir_all(&cache_dir).unwrap();
    let old_file = cache_dir.join("orphan.tmp");
    {
        let mut f = File::create(&old_file).unwrap();
        f.write_all(b"cached orphan bytes").unwrap();
    }

    let catalog = TargetCatalog::new();
    catalog.register_target_simple(
        "android:testapp:cache",
        cache_dir.clone(),
        cache_cleaner_daemon::domain::TargetSafetyTier::StandardCache,
        cache_cleaner_daemon::domain::TargetClass::AppCache,
        "testapp",
    );

    // 2. Commit a job and intent to SQLite
    let job_id = JobId(777);
    let attempt_id = AttemptId(1);
    store.register_job(job_id, "TEST", GenerationId(1), GenerationId(1)).unwrap();
    store.create_attempt(attempt_id, job_id, WorkerId(1), 60).unwrap();

    let intent = OperationIntent::new(
        job_id,
        attempt_id,
        OperationId(1),
        TargetId::new("android:testapp:cache"),
        RelativePath::parse("orphan.tmp").unwrap(),
        FileIdentity::new(1, 1),
        ByteCount::new(100),
        MutationType::DeleteFile,
        GenerationId(1),
        GenerationId(1),
    );
    store.commit_operation_intent(&intent).unwrap();

    // 3. Simulate process crash BEFORE attempt is marked finished
    // File was physically deleted by executor right before the crash
    let _ = fs::remove_file(&old_file);

    // 4. Simulate daemon restart: RecoveryEngine kicks in
    let recovery = RecoveryEngine::new();
    let verifier = PostconditionVerifier::new();

    let recovered_count = recovery
        .reconcile_startup_crashes(&store, &catalog, &verifier)
        .expect("Recovery should succeed");

    assert_eq!(recovered_count, 1);

    // Second recovery run should find 0 uncompleted jobs because it was marked reconciled
    let second_run = recovery
        .reconcile_startup_crashes(&store, &catalog, &verifier)
        .unwrap();
    assert_eq!(second_run, 0);
}

#[test]
fn test_resource_manager_fd_permits_and_exhaustion() {
    let mgr = ResourceManager::new(3, cache_cleaner_daemon::util::rate_limiter::ThrottleMode::Normal); // Limit to 3 concurrent FDs
    assert_eq!(mgr.active_fd_count(), 0);

    let p1 = mgr.acquire_fd_permit().expect("Permit 1");
    let p2 = mgr.acquire_fd_permit().expect("Permit 2");
    let p3 = mgr.acquire_fd_permit().expect("Permit 3");
    assert_eq!(mgr.active_fd_count(), 3);

    // 4th permit must be rejected (pool exhausted)
    assert!(mgr.acquire_fd_permit().is_err());

    // Drop permit 1 -> frees a slot
    drop(p1);
    assert_eq!(mgr.active_fd_count(), 2);

    let p4 = mgr.acquire_fd_permit().expect("Permit 4 should now succeed");
    assert_eq!(mgr.active_fd_count(), 3);

    drop(p2);
    drop(p3);
    drop(p4);
    assert_eq!(mgr.active_fd_count(), 0);
}

#[test]
fn test_scanner_streaming_with_backpressure() {
    let sandbox = TestSandbox::new("streaming_scan");
    let target_dir = sandbox.root.join("target_cache");
    fs::create_dir_all(&target_dir).unwrap();

    // Create 15 temporary files
    for i in 0..15 {
        let fpath = target_dir.join(format!("item_{}.tmp", i));
        let mut f = File::create(&fpath).unwrap();
        writeln!(f, "item {}", i).unwrap();
    }

    let catalog = TargetCatalog::new();
    let _ = catalog.discover_android_user_targets(&sandbox.root);

    let descriptor = cache_cleaner_daemon::domain::target::TargetDescriptor {
        target_id: TargetId::new("test:streaming"),
        target_class: cache_cleaner_daemon::domain::target::TargetClass::AppCache,
        base_path: target_dir,
        dev: DeviceNumber(1),
        ino: InodeNumber(1),
        owner_uid: 0,
        owner_gid: 0,
        package_name: None,
        safety_tier: cache_cleaner_daemon::domain::target::TargetSafetyTier::StandardCache,
        catalog_generation: GenerationId::INITIAL,
    };

    let scanner = CandidateScanner::new();
    let res_mgr = ResourceManager::default();
    let mut chunk_count = 0;
    let mut total_candidates = 0;

    // Scan with chunk size of 5
    scanner.scan_target_streaming(&descriptor, &res_mgr, 5, |chunk| {
        chunk_count += 1;
        total_candidates += chunk.len();
        Ok(true)
    }).unwrap();

    assert_eq!(total_candidates, 15);
    assert_eq!(chunk_count, 3); // 15 / 5 = 3 chunks
}

#[test]
fn test_audit_logger_jsonl_output() {
    let sandbox = TestSandbox::new("audit_logger");
    let audit_path = sandbox.root.join("audit.jsonl");

    let logger = AuditLogger::open_or_create(&audit_path).unwrap();
    let result = JobResult {
        job_id: JobId(101),
        attempt_id: AttemptId(1),
        total_reclaimed: ByteCount::new(1024 * 1024),
        total_operations: 10,
        successful_operations: 10,
        failed_operations: 0,
        skipped_operations: 0,
        duration_ms: 45,
    };

    logger.record_job(&result).unwrap();

    let content = fs::read_to_string(&audit_path).unwrap();
    assert!(content.contains("\"job_id\":101"));
    assert!(content.contains("\"total_reclaimed_bytes\":1048576"));
    assert!(content.contains("\"success\":true"));
}
