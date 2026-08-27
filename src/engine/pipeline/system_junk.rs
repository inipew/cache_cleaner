use super::{CleanStage, PipelineContext};
use crate::engine::rules::JunkCategory;
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

            let path = Path::new(target.path);
            if path.exists() {
                let stats = if target.category == JunkCategory::CrashDump {
                    ctx.walker.clean_crash_dumps_directory(
                        path,
                        ctx.config.cleaning.keep_recent_crash_files,
                    )
                } else {
                    ctx.walker.clean_directory(path)
                };

                report.record_system_junk_stats(&stats, target.category);
            }
        }
    }
}

