use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

use crate::error::{CleanerError, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanParams {
    #[serde(default)]
    pub deep: bool,
    #[serde(default)]
    pub trim: bool,
    #[serde(default)]
    pub zram_compact: bool,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", content = "params")]
pub enum Command {
    Ping,
    TriggerClean(CleanParams),
    GetStatus,
    GetStats,
    GetIdleAssessment,
    Cancel,
    ReloadConfig,
    StopDaemon,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub state: String,
    pub uptime_secs: u64,
    pub last_cleaned_ts: Option<u64>,
    pub last_freed_bytes: u64,
    pub total_freed_bytes: u64,
    pub is_charging: bool,
    pub screen_state: String,
    pub soc_temp_c: f32,
    pub battery_temp_c: f32,
    #[serde(default)]
    pub cpu_usage_pct: f32,
    #[serde(default)]
    pub ram_vm_size_bytes: u64,
    #[serde(default)]
    pub ram_rss_bytes: u64,
    #[serde(default)]
    pub ram_pss_bytes: u64,
    #[serde(default)]
    pub idle_state: String,
    #[serde(default)]
    pub idle_score: u8,
    #[serde(default)]
    pub blockers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StorageReport {
    pub total_freed_bytes: u64,
    pub deleted_files_count: usize,
    pub skipped_files_count: usize,
    pub errors_count: usize,
    pub app_cache_bytes: u64,
    pub oem_logs_bytes: u64,
    pub crash_dumps_bytes: u64,
    pub temp_apks_bytes: u64,
    pub frozen_apps_cleaned: usize,
    pub active_apps_cleaned: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemoryReport {
    pub memory_compacted: bool,
    pub zram_compacted: bool,
    pub cgroup_memory_reclaimed: bool,
    pub reclaimed_mb: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TrimReport {
    pub fstrim_completed: bool,
    pub trimmed_mounts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OptimizationReport {
    pub f2fs_gc_activated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlanEntry {
    pub path: String,
    pub category: String,
    pub size_bytes: u64,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CleanReport {
    pub storage: StorageReport,
    pub memory: MemoryReport,
    pub trim: TrimReport,
    pub optimization: OptimizationReport,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_entries: Option<Vec<PlanEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancel_reason: Option<crate::engine::cancellation::CancelReason>,

    // Flat compatibility fields
    pub total_freed_bytes: u64,
    pub deleted_files_count: usize,
    pub app_cache_freed_bytes: u64,
    pub oem_logs_freed_bytes: u64,
    pub crash_dumps_freed_bytes: u64,
    pub temp_apks_freed_bytes: u64,
    pub skipped_files_count: usize,
    pub errors_count: usize,
    pub frozen_apps_cleaned: usize,
    pub active_apps_cleaned: usize,
    pub memory_compacted: bool,
    pub zram_compacted: bool,
    pub cgroup_memory_reclaimed: bool,
    pub fstrim_completed: bool,
}

impl CleanReport {
    /// Accumulates walk stats for multi-user app cache passes
    pub fn record_app_cache_stats(&mut self, stats: &crate::engine::walker::WalkStats) {
        self.storage.app_cache_bytes += stats.bytes_freed;
        self.storage.total_freed_bytes += stats.bytes_freed;
        self.storage.deleted_files_count += stats.files_deleted;
        self.storage.frozen_apps_cleaned += stats.frozen_apps_affected;
        self.storage.active_apps_cleaned += stats.active_apps_affected;
        self.storage.skipped_files_count += stats.skipped_files;
        self.storage.errors_count += stats.errors_count;

        self.sync_compat_fields();
    }

    /// Accumulates walk stats categorized by system junk category
    pub fn record_system_junk_stats(
        &mut self,
        stats: &crate::engine::walker::WalkStats,
        category: crate::engine::rules::JunkCategory,
    ) {
        match category {
            crate::engine::rules::JunkCategory::OemLog => {
                self.storage.oem_logs_bytes += stats.bytes_freed;
            }
            crate::engine::rules::JunkCategory::CrashDump => {
                self.storage.crash_dumps_bytes += stats.bytes_freed;
            }
            crate::engine::rules::JunkCategory::TempApk => {
                self.storage.temp_apks_bytes += stats.bytes_freed;
            }
            _ => {}
        }

        self.storage.total_freed_bytes += stats.bytes_freed;
        self.storage.deleted_files_count += stats.files_deleted;
        self.storage.skipped_files_count += stats.skipped_files;
        self.storage.errors_count += stats.errors_count;

        self.sync_compat_fields();
    }

    /// Synchronizes flat compatibility fields from nested domain reports
    fn sync_compat_fields(&mut self) {
        self.total_freed_bytes = self.storage.total_freed_bytes;
        self.deleted_files_count = self.storage.deleted_files_count;
        self.app_cache_freed_bytes = self.storage.app_cache_bytes;
        self.oem_logs_freed_bytes = self.storage.oem_logs_bytes;
        self.crash_dumps_freed_bytes = self.storage.crash_dumps_bytes;
        self.temp_apks_freed_bytes = self.storage.temp_apks_bytes;
        self.skipped_files_count = self.storage.skipped_files_count;
        self.errors_count = self.storage.errors_count;
        self.frozen_apps_cleaned = self.storage.frozen_apps_cleaned;
        self.active_apps_cleaned = self.storage.active_apps_cleaned;
        self.memory_compacted = self.memory.memory_compacted;
        self.zram_compacted = self.memory.zram_compacted;
        self.cgroup_memory_reclaimed = self.memory.cgroup_memory_reclaimed;
        self.fstrim_completed = self.trim.fstrim_completed;
    }

    /// Calculates total freed bytes and sets job duration
    pub fn finalize_totals(&mut self, duration_ms: u64) {
        self.sync_compat_fields();
        self.duration_ms = duration_ms;
    }
}


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum Response {
    Success(ResponseData),
    Error(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResponseData {
    Pong { version: String, uptime_secs: u64 },
    Status(DaemonStatus),
    Report(CleanReport),
    Idle(crate::system::idle::IdleAssessment),
    Message(String),
}

pub const MAX_IPC_FRAME_SIZE: usize = 64 * 1024; // 64 KiB max frame size

/// Send a length-prefixed JSON message over a stream
#[allow(dead_code)]
pub fn send_message<W: Write, T: Serialize>(writer: &mut W, message: &T) -> Result<()> {
    let payload = serde_json::to_vec(message)?;
    if payload.len() > MAX_IPC_FRAME_SIZE {
        return Err(CleanerError::Ipc(format!(
            "Outgoing message size {} exceeded 64KB safety limit",
            payload.len()
        )));
    }
    let len = payload.len() as u32;
    writer.write_all(&len.to_be_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()?;
    Ok(())
}

/// Read a length-prefixed JSON message from a stream
#[allow(dead_code)]
pub fn read_message<R: Read, T: for<'de> Deserialize<'de>>(reader: &mut R) -> Result<T> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;

    // Safety limit: 64 KB max for IPC messages
    if len > MAX_IPC_FRAME_SIZE {
        return Err(CleanerError::Ipc(format!(
            "Payload size {} exceeded 64KB safety limit",
            len
        )));
    }

    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload)?;
    let obj = serde_json::from_slice(&payload)?;
    Ok(obj)
}
