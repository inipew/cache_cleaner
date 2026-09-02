use std::fs::{self, File};
use std::io::Write;

use cache_cleaner_daemon::auth::AuthorizationEngine;
use cache_cleaner_daemon::catalog::TargetCatalog;
use cache_cleaner_daemon::config_pipeline::{EffectiveConfig, RawConfig, ValidatedConfig};
use cache_cleaner_daemon::domain::*;
use cache_cleaner_daemon::engine::cancellation::CancellationToken;
use cache_cleaner_daemon::executor::CleanupExecutor;
use cache_cleaner_daemon::planner::CleanupPlanner;
use cache_cleaner_daemon::policy::PolicyEngine;
use cache_cleaner_daemon::safety::SafetyGate;
use cache_cleaner_daemon::scanner::CandidateScanner;
use cache_cleaner_daemon::verifier::PostconditionVerifier;

#[test]
fn test_job_state_machine_deterministic_transitions() {
    let mut attempt = JobAttempt::new(AttemptId(1), JobId(100), 1);
    assert_eq!(attempt.state, JobState::Pending);

    // Valid transitions
    assert!(attempt.transition_to(JobState::Admitted).is_ok());
    assert!(attempt.transition_to(JobState::Scanning).is_ok());
    assert!(attempt.transition_to(JobState::Planning).is_ok());
    assert!(attempt.transition_to(JobState::Authorized).is_ok());
    assert!(attempt.transition_to(JobState::Executing).is_ok());
    assert!(attempt.transition_to(JobState::Verifying).is_ok());
    assert!(attempt.transition_to(JobState::Completed).is_ok());
    assert!(attempt.state.is_terminal());

    // Illegal transition from terminal state
    assert!(attempt.transition_to(JobState::Scanning).is_err());
}

#[test]
fn test_relative_path_safety_rejects_parent_traversal() {
    assert!(RelativePath::parse("/etc/passwd").is_none());
    assert!(RelativePath::parse("../../../etc/passwd").is_none());
    assert!(RelativePath::parse("foo/../bar").is_none());
    assert!(RelativePath::parse("./foo").is_none());

    let valid = RelativePath::parse("cache/sub/item.tmp").expect("Valid relative path");
    assert_eq!(valid.as_str(), "cache/sub/item.tmp");
}

#[test]
fn test_end_to_end_pipeline_execution_and_verification() {
    // 1. Setup isolated test scratch directory mimicking Android app cache
    let temp_root = std::env::temp_dir().join("cache_cleaner_test_e2e");
    let _ = fs::remove_dir_all(&temp_root);
    fs::create_dir_all(&temp_root).unwrap();

    let user_dir = temp_root.join("user_0");
    let pkg_cache = user_dir.join("com.example.testapp").join("cache");
    fs::create_dir_all(&pkg_cache).unwrap();

    // Create old cache file (eligible)
    let old_file = pkg_cache.join("old_cache.tmp");
    {
        let mut f = File::create(&old_file).unwrap();
        f.write_all(b"temporary cache data").unwrap();
    }

    // Create nested directory with cache file
    let sub_dir = pkg_cache.join("webview");
    fs::create_dir_all(&sub_dir).unwrap();
    let webview_cache = sub_dir.join("web.dat");
    {
        let mut f = File::create(&webview_cache).unwrap();
        f.write_all(b"cached web resource").unwrap();
    }

    // 2. Target Catalog Discovery
    let catalog = TargetCatalog::new();
    let count = catalog.discover_android_user_targets(&user_dir).unwrap();
    assert_eq!(count, 1);

    let snapshot = catalog.take_snapshot();
    let target = snapshot
        .get(&TargetId::new("android:com.example.testapp:cache"))
        .expect("Target should be registered");

    // 3. Candidate Scanner Traversal
    let scanner = CandidateScanner::new();
    let candidates = scanner.scan_target(target).unwrap();
    assert_eq!(candidates.len(), 3); // old_file, sub_dir, webview_cache

    // 4. Safety Gate Validation
    let safety = SafetyGate::new();
    let mut safety_validated = Vec::new();
    for cand in candidates {
        let validated = safety.validate_candidate(cand, target).unwrap();
        safety_validated.push(validated);
    }
    assert_eq!(safety_validated.len(), 3);

    // 5. Policy Engine Evaluation
    let val_cfg = ValidatedConfig::from_raw(RawConfig {
        min_app_cache_age_days: Some(0), // Allow immediate cleaning for test
        ..Default::default()
    }).unwrap();
    let eff_cfg = EffectiveConfig::new(snapshot.generation, val_cfg);

    let policy = PolicyEngine::new();
    let mut permits = Vec::new();
    let now = UnixTimestamp::now();

    for sv in safety_validated {
        if let PolicyDecision::Allow(permit) = policy.evaluate_candidate(sv, target, &eff_cfg, now) {
            permits.push(permit);
        }
    }
    assert_eq!(permits.len(), 3);

    // 6. Planner Operation DAG Generation
    let planner = CleanupPlanner::new();
    let planned_plan = planner.build_plan(JobId(42), snapshot.generation, permits);
    assert!(!planned_plan.is_empty());
    assert_eq!(planned_plan.operations.len(), 3);

    // 7. Authorization Capability Grant
    let auth = AuthorizationEngine::new();
    let authorized_plan = auth
        .authorize_plan(planned_plan.clone(), snapshot.generation, 300, GenerationId(1))
        .expect("Authorization should succeed");
    assert!(authorized_plan.is_authorized_for_execution(now, snapshot.generation));

    // Stale generation authorization rejection test
    let stale_gen = GenerationId(999);
    assert!(auth.authorize_plan(planned_plan.clone(), stale_gen, 300, GenerationId(1)).is_err());

    // 8. Executor Single Mutation Execution
    let executor = CleanupExecutor::new();
    let cancel_token = CancellationToken::new();
    let resource_mgr = cache_cleaner_daemon::resource::ResourceManager::default();
    let verifier = PostconditionVerifier::new();
    let safety_gate = SafetyGate::new();

    let job_result = executor
        .execute_plan(
            &authorized_plan,
            &snapshot,
            AttemptId(1),
            &cancel_token,
            &resource_mgr,
            None,
            &safety_gate,
            &verifier,
        )
        .expect("Execution should succeed");

    assert_eq!(job_result.successful_operations, 3);
    assert_eq!(job_result.failed_operations, 0);

    // 9. Verifier Postcondition Checking
    let outcomes = verifier.verify_plan_postcondition(&planned_plan, &snapshot);
    assert_eq!(outcomes.len(), 3);
    for outcome in outcomes.values() {
        assert!(outcome.is_successful_deletion(), "All deleted files must be confirmed absent on disk");
    }

    // Clean up test sandbox
    let _ = fs::remove_dir_all(&temp_root);
}
