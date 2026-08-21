use super::{CleanStage, PipelineContext};
use crate::ipc::protocol::CleanReport;
use crate::platform::{check_encryption_state, enumerate_users, EncryptionState};

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

            let enc_state = check_encryption_state(user.user_id);

            // Clean DE (Device Encrypted) cache (Always accessible)
            if user.de_path.exists() {
                let stats = ctx.walker.clean_directory(&user.de_path);
                report.app_cache_freed_bytes += stats.bytes_freed;
                report.deleted_files_count += stats.files_deleted;
                report.frozen_apps_cleaned += stats.frozen_apps_affected;
                report.active_apps_cleaned += stats.active_apps_affected;
                report.skipped_files_count += stats.skipped_files;
                report.errors_count += stats.errors_count;
            }

            // Clean CE (Credential Encrypted) cache if decrypted
            if enc_state == EncryptionState::FullyUnlocked && user.ce_path.exists() {
                let stats = ctx.walker.clean_directory(&user.ce_path);
                report.app_cache_freed_bytes += stats.bytes_freed;
                report.deleted_files_count += stats.files_deleted;
                report.frozen_apps_cleaned += stats.frozen_apps_affected;
                report.active_apps_cleaned += stats.active_apps_affected;
                report.skipped_files_count += stats.skipped_files;
                report.errors_count += stats.errors_count;
            }

            // Clean External Media cache (/data/media/<id>/Android/data/)
            let ext_data = user.media_path.join("Android/data");
            if ext_data.exists() {
                let stats = ctx.walker.clean_directory(&ext_data);
                report.app_cache_freed_bytes += stats.bytes_freed;
                report.deleted_files_count += stats.files_deleted;
                report.frozen_apps_cleaned += stats.frozen_apps_affected;
                report.active_apps_cleaned += stats.active_apps_affected;
                report.skipped_files_count += stats.skipped_files;
                report.errors_count += stats.errors_count;
            }
        }
    }
}
