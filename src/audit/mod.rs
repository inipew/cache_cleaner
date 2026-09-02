use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use serde::{Deserialize, Serialize};

use crate::domain::result::JobResult;
use crate::domain::types::{JobId, UnixTimestamp};
use crate::error::{CleanerError, Result};

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
    pub fn open_or_create(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(CleanerError::Io)?;

        Ok(Self {
            log_path: path.to_path_buf(),
            writer: Mutex::new(Some(file)),
        })
    }

    pub fn log_path(&self) -> &Path {
        &self.log_path
    }

    pub fn default_logger() -> Result<Self> {
        let primary = Path::new("/data/adb/cleaner/audit/audit.jsonl");
        if primary.parent().is_some_and(|p| p.exists()) {
            Self::open_or_create(primary)
        } else {
            let local_dir = std::env::temp_dir().join("cleaner_audit");
            let _ = fs::create_dir_all(&local_dir);
            Self::open_or_create(&local_dir.join("audit.jsonl"))
        }
    }

    /// Records a completed JobResult into the persistent audit trail.
    pub fn record_job(&self, result: &JobResult) -> Result<()> {
        let record = JobAuditRecord::from(result);
        let mut guard = self.writer.lock().unwrap();
        if let Some(ref mut file) = *guard {
            let json_line = serde_json::to_string(&record)?;
            writeln!(file, "{}", json_line)?;
            file.flush()?;
        }
        Ok(())
    }
}
