use super::{CleanStage, PipelineContext};
use crate::engine::memory::MemoryOptimizer;
use crate::ipc::protocol::CleanReport;

pub struct MemoryOptStage;

impl CleanStage for MemoryOptStage {
    fn name(&self) -> &'static str {
        "MemoryOptimization"
    }

    fn execute(&self, ctx: &PipelineContext, report: &mut CleanReport) {
        if ctx.cancel_token.is_cancelled() || ctx.params.dry_run {
            return;
        }

        // 1. ZRAM & Memory Compaction
        if ctx.params.zram_compact || ctx.config.optimization.zram_compaction {
            if ctx.config.optimization.compact_memory {
                report.memory_compacted = MemoryOptimizer::compact_memory();
            }
            report.zram_compacted = MemoryOptimizer::compact_zram();
        }

        // 2. Cgroup v2 Inactive Memory Reclaim
        if ctx.params.deep || ctx.config.optimization.cgroup_memory_reclaim {
            let target_mb = ctx.config.optimization.cgroup_reclaim_amount_mb;
            report.cgroup_memory_reclaimed = MemoryOptimizer::reclaim_cgroup_memory(target_mb);
        }
    }
}
