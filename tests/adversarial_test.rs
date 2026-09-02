#[cfg(test)]
mod tests {
    use cache_cleaner_daemon::auth::AuthorizationEngine;
    use cache_cleaner_daemon::catalog::TargetCatalog;
    use cache_cleaner_daemon::config_pipeline::{EffectiveConfig, RawConfig, ValidatedConfig};
    use cache_cleaner_daemon::domain::decision::PolicyDecision;
    use cache_cleaner_daemon::domain::types::{AttemptId, GenerationId, JobId, UnixTimestamp};
    use cache_cleaner_daemon::engine::cancellation::CancellationToken;
    use cache_cleaner_daemon::executor::CleanupExecutor;
    use cache_cleaner_daemon::planner::CleanupPlanner;
    use cache_cleaner_daemon::policy::PolicyEngine;
    use cache_cleaner_daemon::safety::SafetyGate;
    use cache_cleaner_daemon::scanner::CandidateScanner;
    use cache_cleaner_daemon::verifier::PostconditionVerifier;

    use std::fs::{self, File};
    use std::io::Write;
    use std::path::PathBuf;

    struct TestSandbox {
        root: PathBuf,
    }

    impl TestSandbox {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("cleaner_test_{}_{}", name, std::process::id()));
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
    fn test_adversarial_symlink_trap_and_data_preservation() {
        let sandbox = TestSandbox::new("symlink_trap");

        // Create directory tree:
        // sandbox/
        // └── user_0/
        //     └── com.example.testapp/
        //         ├── cache/
        //         │   ├── normal_junk.tmp
        //         │   └── symlink_to_databases -> ../databases
        //         ├── databases/
        //         │   └── crucial_data.db
        //         ├── files/
        //         │   └── user_data.json
        //         └── shared_prefs/
        //             └── settings.xml

        let user_dir = sandbox.root.join("user_0");
        let pkg_dir = user_dir.join("com.example.testapp");
        let cache_dir = pkg_dir.join("cache");
        let databases_dir = pkg_dir.join("databases");
        let files_dir = pkg_dir.join("files");
        let prefs_dir = pkg_dir.join("shared_prefs");

        fs::create_dir_all(&cache_dir).unwrap();
        fs::create_dir_all(&databases_dir).unwrap();
        fs::create_dir_all(&files_dir).unwrap();
        fs::create_dir_all(&prefs_dir).unwrap();

        let junk_file = cache_dir.join("normal_junk.tmp");
        let mut f = File::create(&junk_file).unwrap();
        writeln!(f, "junk data").unwrap();

        let db_file = databases_dir.join("crucial_data.db");
        let mut f = File::create(&db_file).unwrap();
        writeln!(f, "CRUCIAL SQLITE DATABASE").unwrap();

        let user_file = files_dir.join("user_data.json");
        let mut f = File::create(&user_file).unwrap();
        writeln!(f, "{{\"token\": \"secret\"}}").unwrap();

        let pref_file = prefs_dir.join("settings.xml");
        let mut f = File::create(&pref_file).unwrap();
        writeln!(f, "<map><string name=\"pref\">value</string></map>").unwrap();

        #[cfg(unix)]
        {
            let trap_symlink = cache_dir.join("symlink_to_databases");
            let _ = std::os::unix::fs::symlink(&databases_dir, &trap_symlink);
        }

        // 1. Target Catalog registers the app cache
        let catalog = TargetCatalog::new();
        let registered = catalog.discover_android_user_targets(&user_dir).unwrap();
        assert_eq!(registered, 1);

        let snapshot = catalog.take_snapshot();
        let target = snapshot.iter().next().expect("Target registered");

        // 2. Candidate Scanner discovers items inside target
        let scanner = CandidateScanner::new();
        let candidates = scanner.scan_target(target).unwrap();

        // 3. Safety Gate & Policy Engine
        let safety = SafetyGate::new();
        let policy = PolicyEngine::new();
        let val_cfg = ValidatedConfig::from_raw(RawConfig {
            min_app_cache_age_days: Some(0),
            ..Default::default()
        }).unwrap();
        let eff_cfg = EffectiveConfig::new(snapshot.generation, val_cfg);
        let now = UnixTimestamp::now();

        let mut permits = Vec::new();
        for cand in candidates {
            if let Ok(validated) = safety.validate_candidate(cand, target) {
                if let PolicyDecision::Allow(permit) = policy.evaluate_candidate(validated, target, &eff_cfg, now) {
                    permits.push(permit);
                }
            }
        }

        // 4. Planner & Auth
        let planner = CleanupPlanner::new();
        let planned = planner.build_plan(JobId(1), snapshot.generation, permits);
        let auth = AuthorizationEngine::new();
        let authorized = auth.authorize_plan(planned.clone(), snapshot.generation, 300, GenerationId(1)).unwrap();

        // 5. Executor & Verifier
        let executor = CleanupExecutor::new();
        let cancel_token = CancellationToken::new();
        let resource_mgr = cache_cleaner_daemon::resource::ResourceManager::default();
        let verifier = PostconditionVerifier::new();
        let safety_gate = SafetyGate::new();

        let result = executor.execute_plan(
            &authorized,
            &snapshot,
            AttemptId(1),
            &cancel_token,
            &resource_mgr,
            None,
            &safety_gate,
            &verifier,
        ).unwrap();
        let _ = verifier.verify_plan_postcondition(&planned, &snapshot);

        // Assertions:
        // 1. Junk in cache must be deleted
        assert!(!junk_file.exists(), "Junk file in cache should have been deleted");

        // 2. Protected directories and their contents MUST NOT be deleted despite symlink trap
        assert!(db_file.exists(), "Databases file MUST NOT be deleted!");
        assert!(user_file.exists(), "User files MUST NOT be deleted!");
        assert!(pref_file.exists(), "Shared preferences MUST NOT be deleted!");

        // Verify content integrity of preserved files
        let db_content = fs::read_to_string(&db_file).unwrap();
        assert_eq!(db_content.trim(), "CRUCIAL SQLITE DATABASE");

        assert!(result.successful_operations >= 1);
    }
}
