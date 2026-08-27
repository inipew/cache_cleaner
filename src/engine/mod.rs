pub mod cancellation;
pub mod framework;
pub mod memory;
pub mod pipeline;
pub mod rules;
pub mod storage;
pub mod walker;

use std::time::Instant;

pub use cancellation::CancellationToken;
pub use pipeline::{CleanPipeline, PipelineContext};
pub use rules::RuleEngine;
pub use storage::StorageOptimizer;
pub use walker::DirectoryWalker;

use crate::config::DaemonConfig;
use crate::hardware::f2fs::F2fsController;
use crate::ipc::protocol::{CleanParams, CleanReport};

pub struct CleanEngine {
    config: DaemonConfig,
    rule_engine: RuleEngine,
    f2fs: F2fsController,
    pipeline: CleanPipeline,
}

impl CleanEngine {
    pub fn new(config: DaemonConfig) -> Self {
        let rule_engine = RuleEngine::new(config.cleaning.clone(), config.safety.clone());
        let f2fs = F2fsController::discover();
        let pipeline = CleanPipeline::build_standard();
        Self {
            config,
            rule_engine,
            f2fs,
            pipeline,
        }
    }

    #[allow(dead_code)]
    pub fn update_config(&mut self, config: DaemonConfig) {
        self.rule_engine = RuleEngine::new(config.cleaning.clone(), config.safety.clone());
        self.config = config;
    }

    pub fn execute(&self, params: &CleanParams, cancel_token: &CancellationToken) -> CleanReport {
        let start_time = Instant::now();
        let mut report = CleanReport::default();

        log::info!(
            "Starting clean job (deep: {}, trim: {}, zram: {}, dry_run: {})",
            params.deep,
            params.trim,
            params.zram_compact,
            params.dry_run
        );

        let frozen_uids = if self.config.optimization.freezer_aware_cleaning {
            Some(crate::system::freezer::enumerate_frozen_uids())
        } else {
            None
        };

        let walker = DirectoryWalker::new(
            &self.rule_engine,
            cancel_token,
            self.config.cleaning.min_file_age_hours,
            params.dry_run,
        )
        .with_frozen_uids(frozen_uids.as_ref());

        let ctx = PipelineContext {
            config: &self.config,
            params,
            cancel_token,
            rule_engine: &self.rule_engine,
            walker: &walker,
            f2fs: &self.f2fs,
            frozen_uids: frozen_uids.as_ref(),
        };

        // Execute all composable clean stages in pipeline
        self.pipeline.execute(&ctx, &mut report);

        let duration_ms = u64::try_from(start_time.elapsed().as_millis()).unwrap_or(u64::MAX);
        report.cancel_reason = cancel_token.get_cancel_reason();
        report.finalize_totals(duration_ms);

        log::info!(
            "Clean job finished in {} ms. Freed: {} MB across {} files (errors: {}, skipped: {})",
            report.duration_ms,
            report.total_freed_bytes / (1024 * 1024),
            report.deleted_files_count,
            report.errors_count,
            report.skipped_files_count,
        );

        report
    }
}

