#[cfg(test)]
mod tests {
    use cache_cleaner_daemon::config::{CleaningRulesConfig, SafetyConfig, SafetyMode};
    use cache_cleaner_daemon::engine::rules::{
        is_valid_package_name, Decision, JunkCategory, RuleEngine, SkipReason,
    };
    use cache_cleaner_daemon::ipc::protocol::{CleanParams, Command};
    #[cfg(unix)]
    use cache_cleaner_daemon::ipc::server::is_command_authorized;
    use cache_cleaner_daemon::platform::{check_encryption_state, EncryptionState, StorageState};
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
    fn test_whitelisted_package_absolute_deny() {
        let rules = CleaningRulesConfig {
            clean_app_cache: true,
            ..Default::default()
        };
        let safety = SafetyConfig {
            mode: SafetyMode::Safe,
            whitelist_packages: vec!["com.google.android.gms".to_string(), "android".to_string()],
            protected_directory_names: vec!["databases".to_string()],
        };
        let engine = RuleEngine::new(rules, safety);

        // Whitelisted package cache MUST produce Decision::Skip (Absolute Deny)
        let cache_path = Path::new("/data/user/0/com.google.android.gms/cache/temp_token.tmp");
        let decision = engine.evaluate_path(cache_path);
        assert!(matches!(
            decision,
            Decision::Skip {
                reason: SkipReason::WhitelistedPackage(ref pkg)
            } if pkg == "com.google.android.gms"
        ));
    }

    #[test]
    fn test_system_immutable_partitions_absolute_deny() {
        let rules = CleaningRulesConfig {
            clean_app_cache: true,
            clean_oem_logs: true,
            clean_temp_apks: true,
            ..Default::default()
        };
        let safety = SafetyConfig {
            mode: SafetyMode::Aggressive,
            ..Default::default()
        };
        let engine = RuleEngine::new(rules, safety);

        // System, vendor, apex, product partitions must ALWAYS produce Decision::Skip
        assert!(matches!(
            engine.evaluate_path(Path::new("/system/app/Chrome/cache/something")),
            Decision::Skip { .. }
        ));
        assert!(matches!(
            engine.evaluate_path(Path::new("/vendor/bin/hw/cache")),
            Decision::Skip { .. }
        ));
        assert!(matches!(
            engine.evaluate_path(Path::new("/apex/com.android.runtime/bin")),
            Decision::Skip { .. }
        ));
    }

    #[test]
    fn test_fail_closed_encryption_state() {
        // Test user with nonexistent path returns unencrypted on test machine or unknown on Android
        let storage_state = StorageState::for_user(99999);
        // On machine where /data doesn't exist, it's Unencrypted for unit testing
        // but if /data exists and user 99999 doesn't, check_encryption_state returns Unknown and ce_available = false
        if !Path::new("/data").exists() {
            assert!(storage_state.ce_available);
        } else {
            let enc_state = check_encryption_state(99999);
            assert_eq!(enc_state, EncryptionState::Unknown);
            assert!(!storage_state.ce_available);
            assert!(!storage_state.user_unlocked);
        }
    }

    #[test]
    fn test_safety_mode_tiers() {
        let rules = CleaningRulesConfig {
            clean_app_cache: true,
            clean_thumbnails: true,
            clean_oem_logs: true,
            clean_crash_dumps: true,
            clean_temp_apks: true,
            ..Default::default()
        };

        // 1. Safe Mode: Thumbnails and OEM logs are blocked
        let safe_cfg = SafetyConfig {
            mode: SafetyMode::Safe,
            ..Default::default()
        };
        let safe_engine = RuleEngine::new(rules.clone(), safe_cfg);
        let thumb_dec = safe_engine.evaluate_path(Path::new("/sdcard/DCIM/.thumbnails/thumb.jpg"));
        assert!(matches!(thumb_dec, Decision::Skip { .. }));
        let oem_dec = safe_engine.evaluate_path(Path::new("/data/mqsas/crash.log"));
        assert!(matches!(oem_dec, Decision::Skip { .. }));

        // 2. Balanced Mode: Thumbnails allowed, OEM logs blocked
        let balanced_cfg = SafetyConfig {
            mode: SafetyMode::Balanced,
            ..Default::default()
        };
        let balanced_engine = RuleEngine::new(rules.clone(), balanced_cfg);
        let thumb_dec = balanced_engine.evaluate_path(Path::new("/sdcard/DCIM/.thumbnails/thumb.jpg"));
        assert!(matches!(thumb_dec, Decision::Delete { category: JunkCategory::Thumbnail, .. }));
        let oem_dec = balanced_engine.evaluate_path(Path::new("/data/mqsas/crash.log"));
        assert!(matches!(oem_dec, Decision::Skip { .. }));

        // 3. Aggressive Mode: Thumbnails and OEM logs both allowed
        let agg_cfg = SafetyConfig {
            mode: SafetyMode::Aggressive,
            ..Default::default()
        };
        let agg_engine = RuleEngine::new(rules, agg_cfg);
        let oem_dec = agg_engine.evaluate_path(Path::new("/data/mqsas/crash.log"));
        assert!(matches!(oem_dec, Decision::Delete { category: JunkCategory::OemLog, .. }));
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

        // System (1000) is authorized for status, ping, and normal clean (NOT Cancel)
        assert!(is_command_authorized(1000, &Command::GetStatus));
        assert!(is_command_authorized(1000, &Command::Ping));
        assert!(!is_command_authorized(1000, &Command::Cancel));
        assert!(is_command_authorized(
            1000,
            &Command::TriggerClean(CleanParams {
                deep: false,
                trim: false,
                zram_compact: false,
                dry_run: false,
            })
        ));

        // System (1000) is NOT authorized for deep clean, cancel, or daemon termination
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

        // Shell (2000) is ONLY authorized for read-only queries (NOT Cancel)
        assert!(is_command_authorized(2000, &Command::GetStatus));
        assert!(is_command_authorized(2000, &Command::GetStats));
        assert!(is_command_authorized(2000, &Command::Ping));
        assert!(!is_command_authorized(2000, &Command::Cancel));
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

    #[test]
    fn test_is_same_or_descendant_boundary_safety() {
        use cache_cleaner_daemon::engine::rules::is_same_or_descendant;

        // Identical match
        assert!(is_same_or_descendant(Path::new("/system"), Path::new("/system")));
        assert!(is_same_or_descendant(Path::new("/data/miui"), Path::new("/data/miui")));

        // True descendants
        assert!(is_same_or_descendant(Path::new("/system/bin/sh"), Path::new("/system")));
        assert!(is_same_or_descendant(Path::new("/data/miui/gallery/log.txt"), Path::new("/data/miui")));

        // Sibling / prefix collisions MUST NOT match
        assert!(!is_same_or_descendant(Path::new("/system_ext"), Path::new("/system")));
        assert!(!is_same_or_descendant(Path::new("/system_ext/app"), Path::new("/system")));
        assert!(!is_same_or_descendant(Path::new("/system_backup"), Path::new("/system")));
        assert!(!is_same_or_descendant(Path::new("/data/user_backup"), Path::new("/data/user")));
        assert!(!is_same_or_descendant(Path::new("/data/miui_backup/log.txt"), Path::new("/data/miui")));
    }

    #[test]
    fn test_oem_directories_are_never_marked_for_deletion() {
        let rules = CleaningRulesConfig {
            clean_oem_logs: true,
            ..Default::default()
        };
        let safety = SafetyConfig {
            mode: SafetyMode::Aggressive,
            ..Default::default()
        };
        let engine = RuleEngine::new(rules, safety);

        // Files with recognized log extensions are marked for delete
        let decision_file = engine.evaluate_path(Path::new("/data/miui/debug.log"));
        assert!(matches!(decision_file, Decision::Delete { category: JunkCategory::OemLog, .. }));

        // Bare directories without log extension must NOT be classified as Delete
        let decision_dir = engine.evaluate_path(Path::new("/data/miui/gallery/subfolder"));
        assert!(!decision_dir.is_delete());
    }
}
