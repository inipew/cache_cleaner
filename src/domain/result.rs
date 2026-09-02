use serde::{Deserialize, Serialize};

use crate::domain::types::{AttemptId, ByteCount, JobId, OperationId, UnixTimestamp};

/// Status of an individual operation after execution and postcondition verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationStatus {
    /// Operation succeeded and postcondition verified on disk
    Success,
    /// Syscall error encountered during mutation
    Failed { error: String },
    /// Syscall claimed success but physical disk check failed
    VerificationFailed { reason: String },
    /// Operation was intentionally skipped
    Skipped { reason: String },
    /// Operation cancelled due to system preemption
    Preempted,
}

impl OperationStatus {
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success)
    }
}

/// Final verified result for a single operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationFinalResult {
    pub op_id: OperationId,
    pub status: OperationStatus,
    pub reclaimed_bytes: ByteCount,
    pub executed_at: UnixTimestamp,
}

/// Aggregated, audited result of a completed job attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobResult {
    pub job_id: JobId,
    pub attempt_id: AttemptId,
    pub total_reclaimed: ByteCount,
    pub total_operations: usize,
    pub successful_operations: usize,
    pub failed_operations: usize,
    pub skipped_operations: usize,
    pub duration_ms: u64,
}
