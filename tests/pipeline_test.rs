#[cfg(test)]
mod tests {
    use cache_cleaner_daemon::config::DaemonConfig;
    use cache_cleaner_daemon::engine::cancellation::CancellationToken;
    use cache_cleaner_daemon::engine::pipeline::{CleanPipeline, PipelineContext};
    use cache_cleaner_daemon::engine::rules::RuleEngine;
    use cache_cleaner_daemon::engine::walker::DirectoryWalker;
    use cache_cleaner_daemon::hardware::f2fs::F2fsController;
    use cache_cleaner_daemon::hardware::telemetry::DeviceEnvironmentSnapshot;
    use cache_cleaner_daemon::ipc::protocol::{CleanParams, CleanReport};

    #[test]
    fn test_pipeline_standard_execution_dry_run() {
        let config = DaemonConfig::default();
        let rule_engine = RuleEngine::new(config.cleaning.clone(), config.safety.clone());
        let cancel_token = CancellationToken::new();
        let f2fs = F2fsController::discover();
        let params = CleanParams {
            deep: false,
            trim: false,
            zram_compact: false,
            dry_run: true,
        };

        let walker = DirectoryWalker::new(&rule_engine, &cancel_token, 0, true);
        let ctx = PipelineContext {
            config: &config,
            params: &params,
            cancel_token: &cancel_token,
            rule_engine: &rule_engine,
            walker: &walker,
            f2fs: &f2fs,
            frozen_uids: None,
        };

        let pipeline = CleanPipeline::build_standard();
        let mut report = CleanReport::default();

        pipeline.execute(&ctx, &mut report);

        assert_eq!(report.errors_count, 0);
        assert!(!report.zram_compacted);
    }

    #[test]
    fn test_pipeline_preemption() {
        let config = DaemonConfig::default();
        let rule_engine = RuleEngine::new(config.cleaning.clone(), config.safety.clone());
        let cancel_token = CancellationToken::new();
        cancel_token.cancel(); // Preempt immediately

        let f2fs = F2fsController::discover();
        let params = CleanParams {
            deep: true,
            trim: true,
            zram_compact: true,
            dry_run: false,
        };

        let walker = DirectoryWalker::new(&rule_engine, &cancel_token, 0, false);
        let ctx = PipelineContext {
            config: &config,
            params: &params,
            cancel_token: &cancel_token,
            rule_engine: &rule_engine,
            walker: &walker,
            f2fs: &f2fs,
            frozen_uids: None,
        };

        let pipeline = CleanPipeline::build_standard();
        let mut report = CleanReport::default();

        pipeline.execute(&ctx, &mut report);

        // Preempted pipeline should not execute subsequent write operations
        assert_eq!(report.deleted_files_count, 0);
        assert!(!report.fstrim_completed);
    }

    #[test]
    fn test_telemetry_snapshot() {
        let snapshot = DeviceEnvironmentSnapshot::capture();
        assert!(snapshot.soc_temp_c >= 0.0 || snapshot.soc_temp_c <= 120.0);
        assert!(snapshot.battery_temp_c >= 0.0 || snapshot.battery_temp_c <= 80.0);
    }
}
