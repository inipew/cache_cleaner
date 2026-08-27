use super::{CleanStage, PipelineContext};
use crate::ipc::protocol::CleanReport;
use crate::platform::{enumerate_users, StorageState};

pub struct AppCacheStage;

impl CleanStage for AppCacheStage {
    fn name(&self) -> &'static str {
        "AppCacheMultiUser"
    }

    fn execute(&self, ctx: &PipelineContext, report: &mut CleanReport) {
        let users = enumerate_users();

        for user in users {
            if ctx.cancel_token.is_cancelled() {
                log::warn!("AppCache stage preempted by cancellation token");
                break;
            }

            let storage_state = StorageState::for_user(user.user_id);

            // Clean DE (Device Encrypted) cache (Always accessible)
            if storage_state.de_available && user.de_path.exists() {
                let stats = ctx.walker.clean_directory(&user.de_path);
                report.record_app_cache_stats(&stats);
            }

            // Clean CE (Credential Encrypted) cache ONLY if storage is decrypted & unlocked
            if storage_state.ce_available && user.ce_path.exists() {
                let stats = ctx.walker.clean_directory(&user.ce_path);
                report.record_app_cache_stats(&stats);
            }

            // Clean External Media cache (/data/media/<id>/Android/data/) only when storage unlocked
            if storage_state.ce_available {
                let ext_data = user.media_path.join("Android/data");
                if ext_data.exists() {
                    let stats = ctx.walker.clean_directory(&ext_data);
                    report.record_app_cache_stats(&stats);
                }
            }
        }
    }
}

