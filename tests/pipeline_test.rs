#[cfg(test)]
mod tests {
    use cache_cleaner_daemon::config::DaemonConfig;
    use cache_cleaner_daemon::engine::cancellation::CancellationToken;
    use cache_cleaner_daemon::engine::CleanEngine;
    use cache_cleaner_daemon::hardware::telemetry::DeviceEnvironmentSnapshot;
    use cache_cleaner_daemon::ipc::protocol::CleanParams;

    #[test]
    fn test_pipeline_standard_execution_dry_run() {
        let config = DaemonConfig::default();
        let mut engine = CleanEngine::new(config);
        let cancel_token = CancellationToken::new();
        let params = CleanParams {
            deep: false,
            trim: false,
            zram_compact: false,
            dry_run: true,
        };

        let report = engine.execute(&params, &cancel_token).unwrap();

        assert_eq!(report.storage.errors_count, 0);
        assert!(!report.memory.zram_compacted);
    }

    #[test]
    fn test_pipeline_preemption() {
        let config = DaemonConfig::default();
        let mut engine = CleanEngine::new(config);
        let cancel_token = CancellationToken::new();
        cancel_token.cancel(); // Preempt immediately

        let params = CleanParams {
            deep: true,
            trim: true,
            zram_compact: true,
            dry_run: false,
        };

        let report = engine.execute(&params, &cancel_token).unwrap();

        // Preempted pipeline should not execute any deletions
        assert_eq!(report.storage.deleted_files_count, 0);
        assert!(!report.trim.fstrim_completed);
    }

    #[test]
    fn test_telemetry_snapshot() {
        let snapshot = DeviceEnvironmentSnapshot::capture();
        assert!(snapshot.soc_temp_c >= 0.0 || snapshot.soc_temp_c <= 120.0);
        assert!(snapshot.battery_temp_c >= 0.0 || snapshot.battery_temp_c <= 80.0);
    }
}
