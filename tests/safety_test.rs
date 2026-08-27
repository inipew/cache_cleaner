#[cfg(test)]
mod tests {
    use cache_cleaner_daemon::config::{CleaningRulesConfig, SafetyConfig};
    use cache_cleaner_daemon::engine::rules::{
        is_valid_package_name, Decision, JunkCategory, RuleEngine, SkipReason,
    };
    use cache_cleaner_daemon::ipc::protocol::{CleanParams, Command};
    #[cfg(unix)]
    use cache_cleaner_daemon::ipc::server::is_command_authorized;
    use std::path::Path;

    #[test]
    fn test_package_name_regex_and_validation() {
        assert!(is_valid_package_name("com.whatsapp"));
        assert!(is_valid_package_name("com.android.chrome"));
        assert!(is_valid_package_name("org.chromium.android_webview"));
        assert!(is_valid_package_name("com.example.app_123"));
        assert!(is_valid_package_name("id.co.bri.brimo"));

        assert!(!is_valid_package_name("plain_string"));
        assert!(!is_valid_package_name("123.com.test"));
        assert!(!is_valid_package_name("com.test..app"));
        assert!(!is_valid_package_name(""));
    }

    #[test]
    fn test_protected_directory_exact_component_matching() {
        let rules = CleaningRulesConfig::default();
        let safety = SafetyConfig::default();
        let engine = RuleEngine::new(rules, safety);

        // Databases, shared_prefs, keystore, fpdata must always produce Decision::Skip
        let decision1 = engine.evaluate_path(Path::new("/data/data/com.whatsapp/databases/msgstore.db"));
        assert!(matches!(
            decision1,
            Decision::Skip {
                reason: SkipReason::ProtectedDirectory(ref d)
            } if d == "databases"
        ));

        let decision2 = engine.evaluate_path(Path::new("/data/data/com.whatsapp/shared_prefs/config.xml"));
        assert!(matches!(
            decision2,
            Decision::Skip {
                reason: SkipReason::ProtectedDirectory(ref d)
            } if d == "shared_prefs"
        ));

        let decision3 = engine.evaluate_path(Path::new("/data/system/users/0/fpdata/settings.bin"));
        assert!(matches!(
            decision3,
            Decision::Skip {
                reason: SkipReason::ProtectedDirectory(ref d)
            } if d == "fpdata"
        ));
    }

    #[test]
    fn test_package_substring_collision_safety() {
        let rules = CleaningRulesConfig {
            clean_app_cache: true,
            ..Default::default()
        };
        let safety = SafetyConfig::default();
        let engine = RuleEngine::new(rules, safety);

        // Package containing substring "database" in package name
        let non_cache = engine.evaluate_path(Path::new(
            "/data/data/com.database.explorer/files/settings.json",
        ));
        assert!(matches!(non_cache, Decision::Skip { .. }));

        // But actual /cache folder is recognized as AppCache
        let cache_file = engine.evaluate_path(Path::new(
            "/data/data/com.database.explorer/cache/query.tmp",
        ));
        assert!(matches!(
            cache_file,
            Decision::Delete {
                category: JunkCategory::AppCache,
                ..
            }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn test_ipc_command_authorization_matrix() {
        // Root (0) is authorized for all commands
        assert!(is_command_authorized(0, &Command::GetStatus));
        assert!(is_command_authorized(0, &Command::StopDaemon));
        assert!(is_command_authorized(0, &Command::ReloadConfig));
        assert!(is_command_authorized(
            0,
            &Command::TriggerClean(CleanParams {
                deep: true,
                trim: true,
                zram_compact: true,
                dry_run: false,
            })
        ));

        // System (1000) is authorized for status, ping, cancel, and normal clean
        assert!(is_command_authorized(1000, &Command::GetStatus));
        assert!(is_command_authorized(1000, &Command::Ping));
        assert!(is_command_authorized(1000, &Command::Cancel));
        assert!(is_command_authorized(
            1000,
            &Command::TriggerClean(CleanParams {
                deep: false,
                trim: false,
                zram_compact: false,
                dry_run: false,
            })
        ));

        // System (1000) is NOT authorized for deep clean or daemon termination
        assert!(!is_command_authorized(
            1000,
            &Command::TriggerClean(CleanParams {
                deep: true,
                trim: false,
                zram_compact: false,
                dry_run: false,
            })
        ));
        assert!(!is_command_authorized(1000, &Command::StopDaemon));

        // Shell (2000) is ONLY authorized for read-only queries & cancel
        assert!(is_command_authorized(2000, &Command::GetStatus));
        assert!(is_command_authorized(2000, &Command::GetStats));
        assert!(is_command_authorized(2000, &Command::Ping));
        assert!(is_command_authorized(2000, &Command::Cancel));
        assert!(!is_command_authorized(
            2000,
            &Command::TriggerClean(CleanParams {
                deep: false,
                trim: false,
                zram_compact: false,
                dry_run: false,
            })
        ));
        assert!(!is_command_authorized(2000, &Command::StopDaemon));
        assert!(!is_command_authorized(2000, &Command::ReloadConfig));

        // Untrusted app UID (10123) is denied for all commands
        assert!(!is_command_authorized(10123, &Command::GetStatus));
        assert!(!is_command_authorized(10123, &Command::StopDaemon));
    }
}
