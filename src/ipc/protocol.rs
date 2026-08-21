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
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CleanReport {
    pub app_cache_freed_bytes: u64,
    pub oem_logs_freed_bytes: u64,
    pub crash_dumps_freed_bytes: u64,
    pub temp_apks_freed_bytes: u64,
    pub total_freed_bytes: u64,
    pub deleted_files_count: usize,
    pub frozen_apps_cleaned: usize,
    pub active_apps_cleaned: usize,
    pub memory_compacted: bool,
    pub zram_compacted: bool,
    pub cgroup_memory_reclaimed: bool,
    pub fstrim_completed: bool,
    pub skipped_files_count: usize,
    pub errors_count: usize,
    pub duration_ms: u64,
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
    Message(String),
}

/// Send a length-prefixed JSON message over a stream
#[allow(dead_code)]
pub fn send_message<W: Write, T: Serialize>(writer: &mut W, message: &T) -> Result<()> {
    let payload = serde_json::to_vec(message)?;
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

    // Safety limit: 256 KB max for IPC messages
    if len > 256 * 1024 {
        return Err(CleanerError::Ipc(format!(
            "Payload size {} exceeded 256KB safety limit",
            len
        )));
    }

    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload)?;
    let obj = serde_json::from_slice(&payload)?;
    Ok(obj)
}
