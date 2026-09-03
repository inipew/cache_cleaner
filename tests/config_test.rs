#[cfg(test)]
mod tests {
    use cache_cleaner_daemon::config::DaemonConfig;

    #[test]
    fn test_default_config_values() {
        let config = DaemonConfig::default();

        assert_eq!(config.maintenance_interval_secs, 21600);
        assert!(config.require_screen_off);
        assert!(config.require_charging_for_deep_clean);
        assert_eq!(config.socket_path, "/data/adb/cleaner/run/daemon");
        assert_eq!(config.abstract_socket_name, "cleaner_daemon");

        assert!(config.cleaning.clean_app_cache);
        assert!(config.cleaning.clean_webview_cache);
        assert!(config.cleaning.clean_image_caches);
        assert!(!config.cleaning.clean_thumbnails); // Conservative default
        assert!(!config.cleaning.clean_oem_logs);   // Conservative default
        assert!(!config.cleaning.clean_crash_dumps); // Conservative default
        assert!(!config.cleaning.clean_temp_apks);   // Conservative default
        assert_eq!(config.cleaning.min_file_age_hours, 24); // 24h default
        assert!(!config.cleaning.clean_code_cache); // Safe default: code_cache disabled

        assert!(!config.optimization.zram_compaction);
        assert!(!config.optimization.compact_memory);
        assert!(!config.optimization.cgroup_memory_reclaim);
        assert_eq!(config.optimization.cgroup_reclaim_amount_mb, 128);
        assert!(config.optimization.freezer_aware_cleaning);
        assert!(config.optimization.psi_adaptive_monitoring);
        assert_eq!(config.optimization.psi_moderate_stall_ms, 150);
        assert_eq!(config.optimization.psi_critical_stall_ms, 250);
        assert_eq!(config.optimization.psi_cooldown_secs, 45);
        assert!(config.optimization.f2fs_gc_urgent);
        assert!(!config.optimization.fstrim_partitions);
    }

    #[test]
    fn test_toml_serialize_deserialize() {
        let config = DaemonConfig::default();
        let toml_str = toml::to_string_pretty(&config).expect("Serialization failed");

        assert!(toml_str.contains("maintenance_interval_secs"));
        assert!(toml_str.contains("clean_app_cache = true"));

        let parsed: DaemonConfig = toml::from_str(&toml_str).expect("Deserialization failed");
        assert_eq!(
            parsed.maintenance_interval_secs,
            config.maintenance_interval_secs
        );
        assert_eq!(parsed.socket_path, config.socket_path);
    }

    #[test]
    fn test_config_validation() {
        let mut config = DaemonConfig::default();
        assert!(config.validate().is_ok());

        // Test invalid interval
        config.maintenance_interval_secs = 10;
        assert!(config.validate().is_err());
        config.maintenance_interval_secs = 3600;

        // Test invalid SoC temp
        config.max_soc_temp_c = 15.0;
        assert!(config.validate().is_err());
        config.max_soc_temp_c = 105.0;
        assert!(config.validate().is_err());
        config.max_soc_temp_c = 45.0;

        // Test empty protected directory names
        config.safety.protected_directory_names.clear();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_strict_config_loading() {
        let tmp_dir = std::env::temp_dir();
        let valid_file = tmp_dir.join("cleaner_valid_test.toml");
        let invalid_file = tmp_dir.join("cleaner_invalid_test.toml");

        let config = DaemonConfig::default();
        config
            .save_to_file(&valid_file)
            .expect("Failed to save valid config");

        let loaded = DaemonConfig::load_from_file_strict(&valid_file);
        assert!(loaded.is_ok());

        std::fs::write(&invalid_file, "this is not valid toml = = =")
            .expect("Failed to write invalid file");
        let invalid_loaded = DaemonConfig::load_from_file_strict(&invalid_file);
        assert!(invalid_loaded.is_err());

        let _ = std::fs::remove_file(valid_file);
        let _ = std::fs::remove_file(invalid_file);
    }

    #[test]
    fn test_config_pipeline_bridging_and_categories() {
        use cache_cleaner_daemon::config_pipeline::{RawConfig, ValidatedConfig};

        // 1. Verify min_file_age_hours conversion to seconds without losing precision
        let raw = RawConfig {
            min_file_age_hours: Some(12),
            whitelist_packages: Some(vec!["com.custom.app".into()]),
            clean_tombstones: Some(true),
            clean_oem_logs: Some(true),
            clean_temp_apks: Some(true),
            ..Default::default()
        };

        let val = ValidatedConfig::from_raw(raw).expect("Validation should succeed");
        assert_eq!(val.min_app_cache_age_secs, 12 * 3600);
        assert_eq!(val.whitelist_packages, vec!["com.custom.app".to_string()]);
        assert!(val.clean_tombstones);
        assert!(val.clean_oem_logs);
        assert!(val.clean_temp_apks);

        // 2. Default fallback when min_file_age_hours is None uses min_app_cache_age_days
        let raw_days = RawConfig {
            min_app_cache_age_days: Some(5),
            min_file_age_hours: None,
            ..Default::default()
        };
        let val_days = ValidatedConfig::from_raw(raw_days).unwrap();
        assert_eq!(val_days.min_app_cache_age_secs, 5 * 86400);
    }
}
