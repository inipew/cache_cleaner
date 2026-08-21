use super::{CleanStage, PipelineContext};
use crate::ipc::protocol::CleanReport;
use std::path::Path;

pub struct SystemJunkStage;

impl CleanStage for SystemJunkStage {
    fn name(&self) -> &'static str {
        "SystemJunkAndLogs"
    }

    fn execute(&self, ctx: &PipelineContext, report: &mut CleanReport) {
        let system_targets = ctx.rule_engine.get_system_junk_targets();

        for target in system_targets {
            if ctx.cancel_token.is_cancelled() {
                break;
            }

            let path = Path::new(target);
            if path.exists() {
                let stats = if target.contains("tombstones")
                    || target.contains("anr")
                    || target.contains("dropbox")
                {
                    ctx.walker.clean_crash_dumps_directory(
                        path,
                        ctx.config.cleaning.keep_recent_crash_files,
                    )
                } else {
                    ctx.walker.clean_directory(path)
                };

                if target.contains("log")
                    || target.contains("miui")
                    || target.contains("oppo")
                    || target.contains("vivo")
                    || target.contains("hilog")
                {
                    report.oem_logs_freed_bytes += stats.bytes_freed;
                } else if target.contains("tombstones")
                    || target.contains("anr")
                    || target.contains("dropbox")
                {
                    report.crash_dumps_freed_bytes += stats.bytes_freed;
                } else if target.contains("app-staging")
                    || target.contains("tmp")
                    || target.contains("package_cache")
                {
                    report.temp_apks_freed_bytes += stats.bytes_freed;
                }

                report.deleted_files_count += stats.files_deleted;
                report.skipped_files_count += stats.skipped_files;
                report.errors_count += stats.errors_count;
            }
        }
    }
}
