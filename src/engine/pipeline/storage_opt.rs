use super::{CleanStage, PipelineContext};
use crate::engine::storage::StorageOptimizer;
use crate::ipc::protocol::CleanReport;

pub struct StorageOptStage;

impl CleanStage for StorageOptStage {
    fn name(&self) -> &'static str {
        "StorageOptimization"
    }

    fn execute(&self, ctx: &PipelineContext, report: &mut CleanReport) {
        if ctx.cancel_token.is_cancelled() || ctx.params.dry_run {
            return;
        }

        // 1. F2FS Background Garbage Collection (scoped)
        let _f2fs_guard = if ctx.f2fs.is_available() && ctx.config.optimization.f2fs_gc_urgent {
            ctx.f2fs.enter_gc_urgent_scoped(2)
        } else {
            None
        };

        // 2. Storage FITRIM
        if ctx.params.trim || ctx.config.optimization.fstrim_partitions {
            log::info!("Running FITRIM on mounted filesystems");
            report.fstrim_completed =
                StorageOptimizer::trim_mounts(&ctx.config.optimization.trim_mount_points);
        }
    }
}
