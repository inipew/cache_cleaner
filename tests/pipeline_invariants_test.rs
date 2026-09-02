use std::fs;
use std::path::PathBuf;

use cache_cleaner_daemon::auth::AuthorizationEngine;
use cache_cleaner_daemon::catalog::TargetCatalog;
use cache_cleaner_daemon::domain::candidate::{Candidate, SafetyValidatedCandidate};
use cache_cleaner_daemon::domain::decision::{DecisionReason, PolicyPermit};
use cache_cleaner_daemon::domain::types::{
    AttemptId, ByteCount, CandidateId, DeviceNumber, FileIdentity, GenerationId, InodeNumber,
    JobId, RelativePath, TargetId, UnixTimestamp,
};
use cache_cleaner_daemon::engine::cancellation::CancellationToken;
use cache_cleaner_daemon::executor::CleanupExecutor;
use cache_cleaner_daemon::fs::SafeDirHandle;
use cache_cleaner_daemon::planner::CleanupPlanner;
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
    let cat_gen = GenerationId(1);

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

    let plan = planner.build_plan(job_id, cat_gen, permits);

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
    );
    let snapshot = catalog.take_snapshot();

    let store = SqliteStore::in_memory().unwrap();
    store.register_job(JobId(1), "TEST", snapshot.generation, GenerationId(1)).unwrap();
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

    let plan = planner.build_plan(JobId(1), snapshot.generation, permits);
    let auth = AuthorizationEngine::new();
    let authorized = auth.authorize_plan(plan, snapshot.generation, 60, GenerationId(1)).unwrap();

    let executor = CleanupExecutor::new();
    let verifier = PostconditionVerifier::new();
    let safety = SafetyGate::new();
    let cancel = CancellationToken::new();
    let resource_mgr = ResourceManager::default();

    let res = executor.execute_plan(
        &authorized,
        &snapshot,
        AttemptId(1),
        &cancel,
        &resource_mgr,
        Some(&store),
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
}
