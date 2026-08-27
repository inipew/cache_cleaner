use super::{CleanStage, PipelineContext};
use crate::engine::storage::StorageOptimizer;
use crate::ipc::protocol::CleanReport;
use std::sync::atomic::{AtomicU64, Ordering};

static LAST_TRIM_UNIX_SECS: AtomicU64 = AtomicU64::new(0);
static BYTES_DELETED_SINCE_LAST_TRIM: AtomicU64 = AtomicU64::new(0);

pub fn record_freed_bytes_for_trim(freed_bytes: u64) {
    BYTES_DELETED_SINCE_LAST_TRIM.fetch_add(freed_bytes, Ordering::SeqCst);
}

pub fn should_run_fstrim(manual_request: bool) -> bool {
    if manual_request {
        return true; // Manual request bypasses cooldown
    }

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let last = LAST_TRIM_UNIX_SECS.load(Ordering::SeqCst);
    let delta_bytes = BYTES_DELETED_SINCE_LAST_TRIM.load(Ordering::SeqCst);

    // Cooldown: at least 24h (86,400s) OR accumulated freed space >= 500 MB
    let time_eligible = last == 0 || (now_secs.saturating_sub(last) >= 86_400);
    let delta_eligible = delta_bytes >= 500_000_000;

    time_eligible || delta_eligible
}

pub fn mark_fstrim_completed() {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    LAST_TRIM_UNIX_SECS.store(now_secs, Ordering::SeqCst);
    BYTES_DELETED_SINCE_LAST_TRIM.store(0, Ordering::SeqCst);
}

pub struct StorageOptStage;

impl CleanStage for StorageOptStage {
    fn name(&self) -> &'static str {
        "StorageOptimization"
    }

    fn execute(&self, ctx: &PipelineContext, report: &mut CleanReport) {
        if ctx.cancel_token.is_cancelled() || ctx.params.dry_run {
            return;
        }

        // Record freed bytes into persistent delta
        if report.storage.total_freed_bytes > 0 {
            record_freed_bytes_for_trim(report.storage.total_freed_bytes);
        }

        // 1. F2FS Background Garbage Collection (scoped)
        let _f2fs_guard = if ctx.f2fs.is_available() && ctx.config.optimization.f2fs_gc_urgent {
            report.optimization.f2fs_gc_activated = true;
            ctx.f2fs.enter_gc_urgent_scoped(2)
        } else {
            None
        };

        // 2. Storage FITRIM with strict cooldown & delta check
        if ctx.params.trim || ctx.config.optimization.fstrim_partitions {
            if should_run_fstrim(ctx.params.trim) {
                log::info!("Running FITRIM on mounted filesystems");
                let ok = StorageOptimizer::trim_mounts(&ctx.config.optimization.trim_mount_points);
                report.trim.fstrim_completed = ok;
                report.fstrim_completed = ok;
                report.trim.trimmed_mounts = ctx.config.optimization.trim_mount_points.clone();
                if ok {
                    mark_fstrim_completed();
                }
            } else {
                log::info!("Skipping FITRIM: 24h cooldown active and accumulated delta < 500 MB");
            }
        }
    }
}
