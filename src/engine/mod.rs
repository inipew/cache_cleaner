pub mod cancellation;
pub mod framework;
pub mod memory;
pub mod rules;
pub mod storage;
pub mod walker;

use std::path::Path;
use std::time::Instant;

pub use cancellation::CancellationToken;
pub use rules::RuleEngine;
pub use storage::StorageOptimizer;
pub use walker::DirectoryWalker;

use crate::config::DaemonConfig;
use crate::hardware::f2fs::F2fsController;
use crate::ipc::protocol::{CleanParams, CleanReport};
use crate::platform::{check_encryption_state, enumerate_users, EncryptionState};

pub struct CleanEngine {
    config: DaemonConfig,
    rule_engine: RuleEngine,
    f2fs: F2fsController,
}

impl CleanEngine {
    pub fn new(config: DaemonConfig) -> Self {
        let rule_engine = RuleEngine::new(config.cleaning.clone(), config.safety.clone());
        let f2fs = F2fsController::discover();
        Self {
            config,
            rule_engine,
            f2fs,
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

        let walker = DirectoryWalker::new(
            &self.rule_engine,
            cancel_token,
            self.config.cleaning.min_file_age_hours,
            params.dry_run,
        );

        // 1. Clean App Caches across all discovered Android users (Multi-User & Private Space)
        let users = enumerate_users();
        for user in users {
            if cancel_token.is_cancelled() {
                log::warn!("Clean job preempted by cancellation token");
                break;
            }

            let enc_state = check_encryption_state(user.user_id);

            // Clean DE (Device Encrypted) cache (Always accessible)
            if user.de_path.exists() {
                let stats = walker.clean_directory(&user.de_path);
                report.app_cache_freed_bytes += stats.bytes_freed;
                report.deleted_files_count += stats.files_deleted;
            }

            // Clean CE (Credential Encrypted) cache if decrypted
            if enc_state == EncryptionState::FullyUnlocked && user.ce_path.exists() {
                let stats = walker.clean_directory(&user.ce_path);
                report.app_cache_freed_bytes += stats.bytes_freed;
                report.deleted_files_count += stats.files_deleted;
            }

            // Clean External Media cache (/data/media/<id>/Android/data/)
            let ext_data = user.media_path.join("Android/data");
            if ext_data.exists() {
                let stats = walker.clean_directory(&ext_data);
                report.app_cache_freed_bytes += stats.bytes_freed;
                report.deleted_files_count += stats.files_deleted;
            }
        }

        // 2. Clean System Junk, OEM Logs, Crash Dumps
        let system_targets = self.rule_engine.get_system_junk_targets();
        for target in system_targets {
            if cancel_token.is_cancelled() {
                break;
            }

            let path = Path::new(target);
            if path.exists() {
                let stats = if target.contains("tombstones") || target.contains("anr") || target.contains("dropbox") {
                    walker.clean_crash_dumps_directory(path, self.config.cleaning.keep_recent_crash_files)
                } else {
                    walker.clean_directory(path)
                };

                if target.contains("log") || target.contains("miui") || target.contains("oppo") || target.contains("vivo") || target.contains("hilog") {
                    report.oem_logs_freed_bytes += stats.bytes_freed;
                } else if target.contains("tombstones") || target.contains("anr") || target.contains("dropbox") {
                    report.crash_dumps_freed_bytes += stats.bytes_freed;
                } else if target.contains("app-staging") || target.contains("tmp") || target.contains("package_cache") {
                    report.temp_apks_freed_bytes += stats.bytes_freed;
                }
                report.deleted_files_count += stats.files_deleted;
            }
        }

        report.total_freed_bytes = report.app_cache_freed_bytes
            + report.oem_logs_freed_bytes
            + report.crash_dumps_freed_bytes
            + report.temp_apks_freed_bytes;

        // 3. Memory & ZRAM Compaction
        if !cancel_token.is_cancelled() {
            if (params.zram_compact || self.config.optimization.zram_compaction) && !params.dry_run {
                if self.config.optimization.compact_memory {
                    report.memory_compacted = memory::MemoryOptimizer::compact_memory();
                }
                report.zram_compacted = memory::MemoryOptimizer::compact_zram();
            }
        }

        // 4. F2FS Background Garbage Collection (scoped with RAII drop guard)
        let _f2fs_guard = if !cancel_token.is_cancelled()
            && self.f2fs.is_available()
            && self.config.optimization.f2fs_gc_urgent
            && !params.dry_run
        {
            self.f2fs.enter_gc_urgent_scoped(2)
        } else {
            None
        };

        // 5. FITRIM Storage Optimization
        if !cancel_token.is_cancelled()
            && (params.trim || self.config.optimization.fstrim_partitions)
            && !params.dry_run
        {
            report.fstrim_completed =
                StorageOptimizer::trim_mounts(&self.config.optimization.trim_mount_points);
        }

        report.duration_ms = start_time.elapsed().as_millis() as u64;

        log::info!(
            "Clean job finished in {} ms. Freed: {} MB across {} files",
            report.duration_ms,
            report.total_freed_bytes / (1024 * 1024),
            report.deleted_files_count
        );

        report
    }

}
