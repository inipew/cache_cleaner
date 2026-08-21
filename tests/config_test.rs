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
        assert!(!config.cleaning.clean_code_cache); // Safe default: code_cache disabled

        assert!(config.optimization.zram_compaction);
        assert!(config.optimization.f2fs_gc_urgent);
        assert!(config.optimization.fstrim_partitions);
    }

    #[test]
    fn test_toml_serialize_deserialize() {
        let config = DaemonConfig::default();
        let toml_str = toml::to_string_pretty(&config).expect("Serialization failed");

        assert!(toml_str.contains("maintenance_interval_secs"));
        assert!(toml_str.contains("clean_app_cache = true"));

        let parsed: DaemonConfig = toml::from_str(&toml_str).expect("Deserialization failed");
        assert_eq!(parsed.maintenance_interval_secs, config.maintenance_interval_secs);
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

        // Test empty protected substrings
        config.safety.protected_substrings.clear();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_strict_config_loading() {
        let tmp_dir = std::env::temp_dir();
        let valid_file = tmp_dir.join("cleaner_valid_test.toml");
        let invalid_file = tmp_dir.join("cleaner_invalid_test.toml");

        let config = DaemonConfig::default();
        config.save_to_file(&valid_file).expect("Failed to save valid config");

        let loaded = DaemonConfig::load_from_file_strict(&valid_file);
        assert!(loaded.is_ok());

        std::fs::write(&invalid_file, "this is not valid toml = = =").expect("Failed to write invalid file");
        let invalid_loaded = DaemonConfig::load_from_file_strict(&invalid_file);
        assert!(invalid_loaded.is_err());

        let _ = std::fs::remove_file(valid_file);
        let _ = std::fs::remove_file(invalid_file);
    }
}
