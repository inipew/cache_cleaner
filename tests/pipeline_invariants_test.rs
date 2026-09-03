use std::fs;
use std::path::PathBuf;

use cache_cleaner_daemon::auth::AuthorizationEngine;
use cache_cleaner_daemon::catalog::TargetCatalog;
use cache_cleaner_daemon::domain::candidate::{Candidate, SafetyValidatedCandidate};
use cache_cleaner_daemon::domain::decision::{DecisionReason, PolicyPermit};
use cache_cleaner_daemon::domain::grant::Capability;
use cache_cleaner_daemon::domain::intent::{MutationType, OperationIntent};
use cache_cleaner_daemon::domain::plan::{OperationType, PlannedOperation, PlannedPlan};
use cache_cleaner_daemon::domain::types::{
    AttemptId, ByteCount, CandidateId, DeviceNumber, FileIdentity, CatalogGeneration, ConfigGeneration, InodeNumber,
    JobId, OperationId, PlanId, RelativePath, TargetId, UnixTimestamp,
};
use cache_cleaner_daemon::engine::cancellation::CancellationToken;
use cache_cleaner_daemon::executor::CleanupExecutor;
use cache_cleaner_daemon::fs::SafeDirHandle;
use cache_cleaner_daemon::planner::CleanupPlanner;
use cache_cleaner_daemon::recovery::RecoveryEngine;
use cache_cleaner_daemon::resource::ResourceManager;
use cache_cleaner_daemon::safety::SafetyGate;
use cache_cleaner_daemon::store::SqliteStore;
use cache_cleaner_daemon::verifier::PostconditionVerifier;

struct TestSandbox {
    root: PathBuf,
}

impl TestSandbox {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("cleaner_pipeline_inv_test_{}_{}", name, std::process::id()));
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
fn test_canonical_hierarchical_dag_dependencies() {
    let planner = CleanupPlanner::new();
    let job_id = JobId(1);
    let cat_gen = CatalogGeneration(1);

    // Create permits for a directory and 2 child files inside it
    let target_id = TargetId::new("test:app");
    let dev = 1u64;

    let dir_cand = Candidate {
        candidate_id: CandidateId(1),
        target_id: target_id.clone(),
        rel_path: RelativePath::parse("cache/images").unwrap(),
        identity: FileIdentity::new(dev, 10),
        size_bytes: ByteCount::ZERO,
        mtime: UnixTimestamp::from_secs(100),
        atime: None,
        is_dir: true,
        is_symlink: false,
    };

    let file_1 = Candidate {
        candidate_id: CandidateId(2),
        target_id: target_id.clone(),
        rel_path: RelativePath::parse("cache/images/pic1.jpg").unwrap(),
        identity: FileIdentity::new(dev, 11),
        size_bytes: ByteCount::new(1024),
        mtime: UnixTimestamp::from_secs(100),
        atime: None,
        is_dir: false,
        is_symlink: false,
    };

    let file_2 = Candidate {
        candidate_id: CandidateId(3),
        target_id: target_id.clone(),
        rel_path: RelativePath::parse("cache/images/pic2.jpg").unwrap(),
        identity: FileIdentity::new(dev, 12),
        size_bytes: ByteCount::new(2048),
        mtime: UnixTimestamp::from_secs(100),
        atime: None,
        is_dir: false,
        is_symlink: false,
    };

    let permits = vec![
        PolicyPermit {
            candidate: SafetyValidatedCandidate::new(dir_cand, DeviceNumber(dev), InodeNumber(1)),
            priority: 100,
            reason: DecisionReason::ExceedsRetentionAge,
            decided_at: UnixTimestamp::now(),
        },
        PolicyPermit {
            candidate: SafetyValidatedCandidate::new(file_1, DeviceNumber(dev), InodeNumber(1)),
            priority: 100,
            reason: DecisionReason::ExceedsRetentionAge,
            decided_at: UnixTimestamp::now(),
        },
        PolicyPermit {
            candidate: SafetyValidatedCandidate::new(file_2, DeviceNumber(dev), InodeNumber(1)),
            priority: 100,
            reason: DecisionReason::ExceedsRetentionAge,
            decided_at: UnixTimestamp::now(),
        },
    ];

    let plan = planner.build_plan(job_id, cat_gen, ConfigGeneration(1), permits).unwrap();

    // Assert files are planned BEFORE the parent directory
    assert_eq!(plan.operations.len(), 3);
    assert!(!plan.operations[0].op_type.is_dir());
    assert!(!plan.operations[1].op_type.is_dir());
    assert!(plan.operations[2].op_type.is_dir());

    // Assert parent directory has explicit dependencies on both child file operations
    let dir_op = &plan.operations[2];
    assert_eq!(dir_op.dependencies.len(), 2);
    assert_eq!(dir_op.dependencies, vec![plan.operations[0].op_id, plan.operations[1].op_id]);
}

#[test]
fn test_durable_intent_commit_and_postcondition_verification() {
    let sandbox = TestSandbox::new("intent_exec");
    let target_dir = sandbox.root.join("app_cache");
    fs::create_dir_all(&target_dir).unwrap();

    let file_path = target_dir.join("temp_file.bin");
    fs::write(&file_path, b"test cache content").unwrap();

    let root_handle = SafeDirHandle::open_root(&target_dir).unwrap();
    let file_id = root_handle.stat_child("temp_file.bin").unwrap();

    let catalog = TargetCatalog::new();
    catalog.register_target_simple(
        "test:pkg",
        target_dir.clone(),
        cache_cleaner_daemon::domain::TargetSafetyTier::StandardCache,
        cache_cleaner_daemon::domain::TargetClass::AppCache,
        "com.test.pkg",
    ).unwrap();
    let snapshot = catalog.take_snapshot();

    let store = SqliteStore::in_memory().unwrap();
    store.register_job(JobId(1), "TEST", snapshot.generation, ConfigGeneration(1)).unwrap();
    store.create_attempt(AttemptId(1), JobId(1), cache_cleaner_daemon::domain::WorkerId(1), 60).unwrap();

    let planner = CleanupPlanner::new();
    let permits = vec![PolicyPermit {
        candidate: SafetyValidatedCandidate::new(
            Candidate {
                candidate_id: CandidateId(1),
                target_id: TargetId::new("test:pkg"),
                rel_path: RelativePath::parse("temp_file.bin").unwrap(),
                identity: file_id,
                size_bytes: ByteCount::new(18),
                mtime: UnixTimestamp::from_secs(100),
                atime: None,
                is_dir: false,
                is_symlink: false,
            },
            file_id.dev,
            InodeNumber(1),
        ),
        priority: 100,
        reason: DecisionReason::ExceedsRetentionAge,
        decided_at: UnixTimestamp::now(),
    }];

    let plan = planner.build_plan(JobId(1), snapshot.generation, ConfigGeneration(1), permits).unwrap();
    let auth = AuthorizationEngine::new();
    let authorized = auth.authorize_plan(plan, snapshot.generation, 60, ConfigGeneration(1)).unwrap();

    let executor = CleanupExecutor::new();
    let verifier = PostconditionVerifier::new();
    let safety = SafetyGate::new();
    let cancel = CancellationToken::new();
    let resource_mgr = ResourceManager::default();

    let res = executor.execute_plan(
        &authorized,
        &snapshot,
        ConfigGeneration(1),
        AttemptId(1),
        &cancel,
        &resource_mgr,
        &store,
        &safety,
        &verifier,
    ).unwrap();

    assert_eq!(res.successful_operations, 1);
    assert_eq!(res.total_reclaimed.as_u64(), 18);
    assert!(!file_path.exists(), "File must be physically deleted from disk");

    // Verify durable intent was recorded in SQLite
    let intents = store.get_operation_intents_for_attempt(AttemptId(1)).unwrap();
    assert_eq!(intents.len(), 1);
    assert_eq!(intents[0].rel_path.as_str(), "temp_file.bin");
    assert_eq!(intents[0].state, cache_cleaner_daemon::domain::intent::IntentState::VerifiedSuccess);
}

#[test]
fn test_dag_dependency_unfulfilled_skips_parent_dir() {
    let sandbox = TestSandbox::new("dag_skip");
    let target_dir = sandbox.root.join("app_cache");
    let sub_dir = target_dir.join("temp_dir");
    fs::create_dir_all(&sub_dir).unwrap();

    // Create a child file inside subdir
    let child_file = sub_dir.join("child.bin");
    fs::write(&child_file, b"child bytes").unwrap();

    let root_handle = SafeDirHandle::open_root(&target_dir).unwrap();
    let file_id = root_handle.open_child_dir("temp_dir").unwrap().stat_child("child.bin").unwrap();
    let dir_id = root_handle.stat_child("temp_dir").unwrap();

    let catalog = TargetCatalog::new();
    catalog.register_target_simple(
        "test:pkg",
        target_dir.clone(),
        cache_cleaner_daemon::domain::TargetSafetyTier::StandardCache,
        cache_cleaner_daemon::domain::TargetClass::AppCache,
        "com.test.pkg",
    ).unwrap();
    let snapshot = catalog.take_snapshot();

    let store = SqliteStore::in_memory().unwrap();
    store.register_job(JobId(1), "TEST", snapshot.generation, ConfigGeneration(1)).unwrap();
    store.create_attempt(AttemptId(1), JobId(1), cache_cleaner_daemon::domain::WorkerId(1), 60).unwrap();

    // Create a plan where child file has WRONG expected identity (will fail TOCTOU)
    // and parent directory depends on this child op
    let child_op = PlannedOperation {
        op_id: OperationId(1),
        op_type: OperationType::DeleteFile {
            target_id: TargetId::new("test:pkg"),
            rel_path: RelativePath::parse("temp_dir/child.bin").unwrap(),
            expected_identity: FileIdentity::new(file_id.dev.0, file_id.ino.0 + 9999), // Identity mismatch!
            estimated_size: ByteCount::new(11),
        },
        dependencies: vec![],
        estimated_reclaim: ByteCount::new(11),
    };

    let dir_op = PlannedOperation {
        op_id: OperationId(2),
        op_type: OperationType::DeleteDirEmpty {
            target_id: TargetId::new("test:pkg"),
            rel_path: RelativePath::parse("temp_dir").unwrap(),
            expected_identity: dir_id,
        },
        dependencies: vec![OperationId(1)], // Dependent on Op 1!
        estimated_reclaim: ByteCount::ZERO,
    };

    let plan = PlannedPlan {
        plan_id: PlanId(1),
        job_id: JobId(1),
        catalog_generation: snapshot.generation,
        config_generation: ConfigGeneration(1),
        operations: vec![child_op, dir_op],
        total_estimated_reclaim: ByteCount::new(11),
        created_at: UnixTimestamp::now(),
    };

    let auth = AuthorizationEngine::new();
    let authorized = auth.authorize_plan(plan, snapshot.generation, 60, ConfigGeneration(1)).unwrap();

    let executor = CleanupExecutor::new();
    let verifier = PostconditionVerifier::new();
    let safety = SafetyGate::new();
    let cancel = CancellationToken::new();
    let resource_mgr = ResourceManager::default();

    let res = executor.execute_plan(
        &authorized,
        &snapshot,
        ConfigGeneration(1),
        AttemptId(1),
        &cancel,
        &resource_mgr,
        &store,
        &safety,
        &verifier,
    ).unwrap();

    // Child op must fail (TOCTOU)
    assert_eq!(res.failed_operations, 1);
    // Dir op MUST be skipped due to unfulfilled dependency!
    assert_eq!(res.skipped_operations, 1);
    assert_eq!(res.successful_operations, 0);

    // Directory and child file MUST still exist on disk!
    assert!(child_file.exists());
    assert!(sub_dir.exists());
}

#[test]
fn test_capability_grant_enforcement_rejection() {
    let sandbox = TestSandbox::new("cap_rejection");
    let target_dir = sandbox.root.join("app_cache");
    fs::create_dir_all(&target_dir).unwrap();

    let file_path = target_dir.join("unauthorized.bin");
    fs::write(&file_path, b"secret data").unwrap();

    let root_handle = SafeDirHandle::open_root(&target_dir).unwrap();
    let file_id = root_handle.stat_child("unauthorized.bin").unwrap();

    let catalog = TargetCatalog::new();
    catalog.register_target_simple(
        "test:pkg",
        target_dir.clone(),
        cache_cleaner_daemon::domain::TargetSafetyTier::StandardCache,
        cache_cleaner_daemon::domain::TargetClass::AppCache,
        "com.test.pkg",
    ).unwrap();
    let snapshot = catalog.take_snapshot();

    let store = SqliteStore::in_memory().unwrap();
    store.register_job(JobId(1), "TEST", snapshot.generation, ConfigGeneration(1)).unwrap();
    store.create_attempt(AttemptId(1), JobId(1), cache_cleaner_daemon::domain::WorkerId(1), 60).unwrap();

    let plan = PlannedPlan {
        plan_id: PlanId(1),
        job_id: JobId(1),
        catalog_generation: snapshot.generation,
        config_generation: ConfigGeneration(1),
        operations: vec![PlannedOperation {
            op_id: OperationId(1),
            op_type: OperationType::DeleteFile {
                target_id: TargetId::new("test:pkg"),
                rel_path: RelativePath::parse("unauthorized.bin").unwrap(),
                expected_identity: file_id,
                estimated_size: ByteCount::new(11),
            },
            dependencies: vec![],
            estimated_reclaim: ByteCount::new(11),
        }],
        total_estimated_reclaim: ByteCount::new(11),
        created_at: UnixTimestamp::now(),
    };

    // Construct an AuthorizedPlan with an EMPTY capability grant (missing DeleteFile permission)
    let grant = cache_cleaner_daemon::domain::grant::CapabilityGrant {
        grant_id: cache_cleaner_daemon::domain::types::GrantId(1),
        capabilities: vec![Capability::ReadTarget(TargetId::new("test:pkg"))], // NO DeleteFile capability!
        catalog_generation: snapshot.generation,
        config_generation: ConfigGeneration(1),
        granted_at: UnixTimestamp::now(),
        expires_at: UnixTimestamp::from_secs(UnixTimestamp::now().as_secs() + 60),
    };

    let authorized = cache_cleaner_daemon::domain::grant::AuthorizedPlan {
        plan,
        grant,
    };

    let executor = CleanupExecutor::new();
    let verifier = PostconditionVerifier::new();
    let safety = SafetyGate::new();
    let cancel = CancellationToken::new();
    let resource_mgr = ResourceManager::default();

    let res = executor.execute_plan(
        &authorized,
        &snapshot,
        ConfigGeneration(1),
        AttemptId(1),
        &cancel,
        &resource_mgr,
        &store,
        &safety,
        &verifier,
    ).unwrap();

    // Executor must fail the operation closed due to missing capability grant
    assert_eq!(res.failed_operations, 1);
    assert_eq!(res.successful_operations, 0);
    // File must NOT be deleted!
    assert!(file_path.exists());
}

#[test]
fn test_planner_target_scoping_no_cross_collision() {
    let planner = CleanupPlanner::new();
    let job_id = JobId(10);
    let cat_gen = CatalogGeneration(1);

    let target_a = TargetId::new("target:app_a");
    let target_b = TargetId::new("target:app_b");

    // Both targets have identical relative paths: "cache/images" (dir) and "cache/images/pic.jpg" (file)
    let permits = vec![
        PolicyPermit {
            candidate: SafetyValidatedCandidate::new(
                Candidate {
                    candidate_id: CandidateId(1),
                    target_id: target_a.clone(),
                    rel_path: RelativePath::parse("cache/images").unwrap(),
                    identity: FileIdentity::new(1, 10),
                    size_bytes: ByteCount::ZERO,
                    mtime: UnixTimestamp::from_secs(100),
                    atime: None,
                    is_dir: true,
                    is_symlink: false,
                },
                DeviceNumber(1),
                InodeNumber(1),
            ),
            priority: 100,
            reason: DecisionReason::ExceedsRetentionAge,
            decided_at: UnixTimestamp::now(),
        },
        PolicyPermit {
            candidate: SafetyValidatedCandidate::new(
                Candidate {
                    candidate_id: CandidateId(2),
                    target_id: target_a.clone(),
                    rel_path: RelativePath::parse("cache/images/pic.jpg").unwrap(),
                    identity: FileIdentity::new(1, 11),
                    size_bytes: ByteCount::new(100),
                    mtime: UnixTimestamp::from_secs(100),
                    atime: None,
                    is_dir: false,
                    is_symlink: false,
                },
                DeviceNumber(1),
                InodeNumber(1),
            ),
            priority: 100,
            reason: DecisionReason::ExceedsRetentionAge,
            decided_at: UnixTimestamp::now(),
        },
        PolicyPermit {
            candidate: SafetyValidatedCandidate::new(
                Candidate {
                    candidate_id: CandidateId(3),
                    target_id: target_b.clone(),
                    rel_path: RelativePath::parse("cache/images").unwrap(),
                    identity: FileIdentity::new(1, 20),
                    size_bytes: ByteCount::ZERO,
                    mtime: UnixTimestamp::from_secs(100),
                    atime: None,
                    is_dir: true,
                    is_symlink: false,
                },
                DeviceNumber(1),
                InodeNumber(1),
            ),
            priority: 100,
            reason: DecisionReason::ExceedsRetentionAge,
            decided_at: UnixTimestamp::now(),
        },
        PolicyPermit {
            candidate: SafetyValidatedCandidate::new(
                Candidate {
                    candidate_id: CandidateId(4),
                    target_id: target_b.clone(),
                    rel_path: RelativePath::parse("cache/images/pic.jpg").unwrap(),
                    identity: FileIdentity::new(1, 21),
                    size_bytes: ByteCount::new(200),
                    mtime: UnixTimestamp::from_secs(100),
                    atime: None,
                    is_dir: false,
                    is_symlink: false,
                },
                DeviceNumber(1),
                InodeNumber(1),
            ),
            priority: 100,
            reason: DecisionReason::ExceedsRetentionAge,
            decided_at: UnixTimestamp::now(),
        },
    ];

    let plan = planner.build_plan(job_id, cat_gen, ConfigGeneration(1), permits).unwrap();
    assert_eq!(plan.operations.len(), 4);

    // Find directory operations for Target A and Target B
    let dir_a = plan.operations.iter().find(|op| match &op.op_type {
        OperationType::DeleteDirEmpty { target_id, .. } => target_id == &target_a,
        _ => false,
    }).unwrap();

    let dir_b = plan.operations.iter().find(|op| match &op.op_type {
        OperationType::DeleteDirEmpty { target_id, .. } => target_id == &target_b,
        _ => false,
    }).unwrap();

    let file_a = plan.operations.iter().find(|op| match &op.op_type {
        OperationType::DeleteFile { target_id, .. } => target_id == &target_a,
        _ => false,
    }).unwrap();

    let file_b = plan.operations.iter().find(|op| match &op.op_type {
        OperationType::DeleteFile { target_id, .. } => target_id == &target_b,
        _ => false,
    }).unwrap();

    // Directory A MUST ONLY depend on File A
    assert_eq!(dir_a.dependencies, vec![file_a.op_id]);
    // Directory B MUST ONLY depend on File B
    assert_eq!(dir_b.dependencies, vec![file_b.op_id]);
}

#[test]
fn test_recovery_unresolved_unknown_state_precision() {
    let sandbox = TestSandbox::new("recovery_precision");
    let target_dir = sandbox.root.join("testapp").join("cache");
    fs::create_dir_all(&target_dir).unwrap();

    let db_path = sandbox.root.join("recovery.db");
    let store = SqliteStore::open_or_create(&db_path).unwrap();

    let catalog = TargetCatalog::new();
    catalog.register_target_simple(
        "android:testapp:cache",
        target_dir.clone(),
        cache_cleaner_daemon::domain::TargetSafetyTier::StandardCache,
        cache_cleaner_daemon::domain::TargetClass::AppCache,
        "testapp",
    ).unwrap();

    let job_id = JobId(500);
    let attempt_id = AttemptId(1);
    store.register_job(job_id, "TEST", CatalogGeneration(1), ConfigGeneration(1)).unwrap();
    store.create_attempt(attempt_id, job_id, cache_cleaner_daemon::domain::WorkerId(1), 60).unwrap();

    // Intent pointing to a target that is NOT registered in the catalog (simulates unmounted/missing storage)
    let missing_intent = OperationIntent::new(
        job_id,
        attempt_id,
        OperationId(1),
        TargetId::new("android:missing_unmounted:cache"),
        RelativePath::parse("file.tmp").unwrap(),
        FileIdentity::new(1, 1),
        ByteCount::new(100),
        MutationType::DeleteFile,
        CatalogGeneration(1),
        ConfigGeneration(1),
    );
    store.commit_operation_intent(&missing_intent).unwrap();

    let recovery = RecoveryEngine::new();
    let verifier = PostconditionVerifier::new();

    let recovered = recovery.reconcile_startup_crashes(&store, &catalog, &verifier).unwrap();

    // Because an intent had an unknown outcome, recovered count is 0 (not fully recovered)
    assert_eq!(recovered, 0);

    // Attempt must be marked UNRESOLVED in database, not RECONCILED!
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let state: String = conn.query_row("SELECT state FROM attempts WHERE attempt_id = 1;", [], |r| r.get(0)).unwrap();
    assert_eq!(state, "UNRESOLVED");

    let job_state: String = conn.query_row("SELECT state FROM jobs WHERE job_id = 500;", [], |r| r.get(0)).unwrap();
    assert_eq!(job_state, "PARTIALLY_RECONCILED");
}

#[test]
fn test_authorized_plan_stale_config_generation_rejection() {
    let catalog = TargetCatalog::new();
    let snapshot = catalog.take_snapshot();

    let plan = PlannedPlan {
        plan_id: PlanId(1),
        job_id: JobId(1),
        catalog_generation: snapshot.generation,
        config_generation: ConfigGeneration(1),
        operations: vec![],
        total_estimated_reclaim: ByteCount::ZERO,
        created_at: UnixTimestamp::now(),
    };

    let auth = AuthorizationEngine::new();
    // Authorized with ConfigGeneration 1
    let authorized = auth.authorize_plan(plan, snapshot.generation, 60, ConfigGeneration(1)).unwrap();

    let now = UnixTimestamp::now();
    // Valid when current config generation is 1
    assert!(authorized.is_authorized_for_execution(now, snapshot.generation, ConfigGeneration(1)));

    // REJECTED when config has reloaded to ConfigGeneration 2!
    assert!(!authorized.is_authorized_for_execution(now, snapshot.generation, ConfigGeneration(2)));
}

#[test]
fn test_register_target_simple_fails_on_nonexistent_path() {
    let catalog = TargetCatalog::new();
    let res = catalog.register_target_simple(
        "invalid:target",
        PathBuf::from("/nonexistent/directory/that/never/exists/12345"),
        cache_cleaner_daemon::domain::TargetSafetyTier::StandardCache,
        cache_cleaner_daemon::domain::TargetClass::AppCache,
        "invalid",
    );

    // MUST return Err and not fabricate fake dev=1 ino=1 identity!
    assert!(res.is_err());
}

#[test]
fn test_external_storage_and_multiuser_discovery() {
    let tmp = std::env::temp_dir().join(format!("test_ext_discovery_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::create_dir_all(&tmp);

    // Mock media directory: <media_base>/Android/data/<pkg>/cache
    let pkg_cache = tmp.join("Android/data/com.example.game/cache");
    std::fs::create_dir_all(&pkg_cache).unwrap();
    std::fs::write(pkg_cache.join("cached_asset.bin"), b"asset_data").unwrap();

    let catalog = TargetCatalog::new();
    let discovered = catalog.discover_android_external_targets(&tmp, 10).unwrap();
    assert_eq!(discovered, 1);

    let snapshot = catalog.take_snapshot();
    let target_id = TargetId::new("android:u10:com.example.game:ext_cache");
    let target = snapshot.get(&target_id).expect("External cache target should be registered");
    assert_eq!(target.target_class, cache_cleaner_daemon::domain::TargetClass::ExternalCache);
    assert_eq!(target.package_name, Some("com.example.game".to_string()));

    let _ = std::fs::remove_dir_all(&tmp);
}
