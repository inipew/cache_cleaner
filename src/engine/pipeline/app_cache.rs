use std::fs;
use std::path::Path;

use super::{CleanStage, PipelineContext};
use crate::ipc::protocol::CleanReport;
use crate::platform::{enumerate_users, StorageState};

pub struct AppCacheStage;

const KNOWN_CACHE_SUBDIRS: &[&str] = &[
    "cache",
    "code_cache",
    ".cache",
    "app_webview/Default/Cache",
    "app_webview/Default/Code Cache",
    "app_webview/Default/GPUCache",
    "app_webview/Default/Service Worker/CacheStorage",
    "app_webview/Default/Service Worker/ScriptCache",
    "app_textures",
    "splash_cache",
    "image_cache",
    "fresco_cache",
    "glide_cache",
    "coil_cache",
    "http-engine-cache",
    "picasso-cache",
    "volley",
    "disk_cache",
    "network_cache",
];

fn clean_user_packages_cache(
    base_dir: &Path,
    ctx: &PipelineContext,
    report: &mut CleanReport,
) {
    if !base_dir.exists() {
        return;
    }

    if let Ok(entries) = fs::read_dir(base_dir) {
        for entry in entries.flatten() {
            if ctx.cancel_token.is_cancelled() {
                break;
            }

            let pkg_path = entry.path();
            if !pkg_path.is_dir() {
                continue;
            }

            // Target strictly known cache subdirectories within each package
            for cache_sub in KNOWN_CACHE_SUBDIRS {
                if ctx.cancel_token.is_cancelled() {
                    break;
                }

                let target = pkg_path.join(cache_sub);
                if target.exists() && target.is_dir() {
                    let stats = ctx.walker.clean_directory(&target);
                    report.record_app_cache_stats(&stats);
                }
            }
        }
    }
}

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

            // Clean DE (Device Encrypted) cache packages (Always accessible)
            if storage_state.de_available {
                clean_user_packages_cache(&user.de_path, ctx, report);
            }

            // Clean CE (Credential Encrypted) cache packages ONLY if storage is decrypted & unlocked
            if storage_state.ce_available {
                clean_user_packages_cache(&user.ce_path, ctx, report);

                // Clean External Media cache (/data/media/<id>/Android/data/<pkg>/cache/) strictly per package
                let ext_data = user.media_path.join("Android/data");
                clean_user_packages_cache(&ext_data, ctx, report);
            }
        }
    }
}

