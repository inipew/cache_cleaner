use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use serde::{Deserialize, Serialize};

use crate::domain::result::JobResult;
use crate::domain::types::{JobId, UnixTimestamp};
use crate::error::{CleanerError, Result};

pub const MAX_AUDIT_LOG_SIZE_BYTES: u64 = 10 * 1024 * 1024; // 10 MB

/// Persistent structured audit record for a completed or failed cleanup job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobAuditRecord {
    pub job_id: JobId,
    pub attempt_id: u64,
    pub recorded_at: UnixTimestamp,
    pub duration_ms: u64,
    pub total_reclaimed_bytes: u64,
    pub operations_total: usize,
    pub operations_successful: usize,
    pub operations_failed: usize,
    pub operations_skipped: usize,
    pub success: bool,
}

impl From<&JobResult> for JobAuditRecord {
    fn from(result: &JobResult) -> Self {
        Self {
            job_id: result.job_id,
            attempt_id: result.attempt_id.0,
            recorded_at: UnixTimestamp::now(),
            duration_ms: result.duration_ms,
            total_reclaimed_bytes: result.total_reclaimed.as_u64(),
            operations_total: result.total_operations,
            operations_successful: result.successful_operations,
            operations_failed: result.failed_operations,
            operations_skipped: result.skipped_operations,
            success: result.failed_operations == 0,
        }
    }
}

/// Audit Logger appending structured audit records in JSON Lines format to disk.
#[derive(Debug)]
pub struct AuditLogger {
    log_path: PathBuf,
    writer: Mutex<Option<File>>,
}

impl AuditLogger {
    /// Opens or creates audit file enforcing 0600 permissions and O_NOFOLLOW (Spec 91.md).
    pub fn open_or_create(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(CleanerError::Io)?;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }

        let mut opts = OpenOptions::new();
        opts.create(true)
            .append(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .mode(0o600);

        let file = opts.open(path).map_err(CleanerError::Io)?;

        Ok(Self {
            log_path: path.to_path_buf(),
            writer: Mutex::new(Some(file)),
        })
    }

    pub fn log_path(&self) -> &Path {
        &self.log_path
    }

    /// Primary production audit log path without unsafe world-writable fallback (Spec 91.md).
    pub fn default_logger() -> Result<Self> {
        let primary = Path::new("/data/adb/cleaner/audit/audit.jsonl");
        Self::open_or_create(primary)
    }

    /// Records a completed JobResult into the persistent audit trail with fsync durability.
    pub fn record_job(&self, result: &JobResult) -> Result<()> {
        let record = JobAuditRecord::from(result);
        let mut guard = self.writer.lock().unwrap();

        // 10MB Log Rotation (Spec 91.md)
        if let Ok(meta) = fs::metadata(&self.log_path) {
            if meta.len() >= MAX_AUDIT_LOG_SIZE_BYTES {
                drop(guard.take());
                let rotated = self.log_path.with_extension("jsonl.1");
                let _ = fs::rename(&self.log_path, &rotated);

                let mut opts = OpenOptions::new();
                opts.create(true)
                    .append(true)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                    .mode(0o600);
                if let Ok(new_file) = opts.open(&self.log_path) {
                    *guard = Some(new_file);
                }
            }
        }

        if let Some(ref mut file) = *guard {
            let json_line = serde_json::to_string(&record)?;
            writeln!(file, "{}", json_line)?;
            file.sync_all()?;
        }
        Ok(())
    }
}
