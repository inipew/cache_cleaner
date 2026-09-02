use serde::{Deserialize, Serialize};
use std::fmt;

use crate::domain::types::{
    AttemptId, ByteCount, FileIdentity, GenerationId, JobId, OperationId, RelativePath, TargetId,
    UnixTimestamp,
};

/// Type of filesystem mutation to be performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MutationType {
    DeleteFile,
    DeleteDirEmpty,
    PruneDirRecursive,
}

impl fmt::Display for MutationType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MutationType::DeleteFile => write!(f, "DELETE_FILE"),
            MutationType::DeleteDirEmpty => write!(f, "DELETE_DIR_EMPTY"),
            MutationType::PruneDirRecursive => write!(f, "PRUNE_DIR_RECURSIVE"),
        }
    }
}

/// Durable Operation Intent recorded and committed to the store BEFORE physical mutation.
/// Guarantees exact idempotency, crash consistency, and post-crash reconciliation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationIntent {
    pub intent_id: Option<i64>,
    pub job_id: JobId,
    pub attempt_id: AttemptId,
    pub op_id: OperationId,
    pub target_id: TargetId,
    pub rel_path: RelativePath,
    pub expected_identity: FileIdentity,
    pub estimated_bytes: ByteCount,
    pub mutation_type: MutationType,
    pub catalog_generation: GenerationId,
    pub config_generation: GenerationId,
    pub committed_at: UnixTimestamp,
}

impl OperationIntent {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        job_id: JobId,
        attempt_id: AttemptId,
        op_id: OperationId,
        target_id: TargetId,
        rel_path: RelativePath,
        expected_identity: FileIdentity,
        estimated_bytes: ByteCount,
        mutation_type: MutationType,
        catalog_generation: GenerationId,
        config_generation: GenerationId,
    ) -> Self {
        Self {
            intent_id: None,
            job_id,
            attempt_id,
            op_id,
            target_id,
            rel_path,
            expected_identity,
            estimated_bytes,
            mutation_type,
            catalog_generation,
            config_generation,
            committed_at: UnixTimestamp::now(),
        }
    }
}
