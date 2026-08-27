#[cfg(test)]
mod tests {
    use cache_cleaner_daemon::config::{CleaningRulesConfig, SafetyConfig};
    use cache_cleaner_daemon::engine::cancellation::CancellationToken;
    use cache_cleaner_daemon::engine::rules::RuleEngine;
    use cache_cleaner_daemon::engine::walker::DirectoryWalker;
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
        // ├── cache/
        // │   ├── normal_junk.tmp
        // │   └── symlink_to_important -> sandbox/databases
        // ├── databases/
        // │   └── crucial_data.db
        // ├── files/
        // │   └── user_data.json
        // └── shared_prefs/
        //     └── settings.xml

        let cache_dir = sandbox.root.join("cache");
        let databases_dir = sandbox.root.join("databases");
        let files_dir = sandbox.root.join("files");
        let prefs_dir = sandbox.root.join("shared_prefs");

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
            let trap_symlink = cache_dir.join("symlink_to_important");
            let _ = std::os::unix::fs::symlink(&databases_dir, &trap_symlink);
        }

        let rules = CleaningRulesConfig {
            clean_app_cache: true,
            min_file_age_hours: 0,
            ..Default::default()
        };
        let safety = SafetyConfig::default();
        let engine = RuleEngine::new(rules, safety);
        let cancel_token = CancellationToken::new();
        let walker = DirectoryWalker::new(&engine, &cancel_token, 0, false);

        // Run clean pass on sandbox root
        let stats = walker.clean_directory(&sandbox.root);

        // 1. Junk in cache must be deleted
        assert!(!junk_file.exists(), "Junk file in cache should have been deleted");

        // 2. Protected directories and their contents MUST NOT be deleted
        assert!(db_file.exists(), "Databases file MUST NOT be deleted!");
        assert!(user_file.exists(), "User files MUST NOT be deleted!");
        assert!(pref_file.exists(), "Shared preferences MUST NOT be deleted!");

        // Verify content integrity of preserved files
        let db_content = fs::read_to_string(&db_file).unwrap();
        assert_eq!(db_content.trim(), "CRUCIAL SQLITE DATABASE");

        assert!(stats.files_deleted >= 1);
    }
}
