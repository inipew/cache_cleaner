use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::domain::types::{
    ByteCount, CatalogGeneration, ConfigGeneration, FileIdentity, JobId, OperationId, PlanId,
    RelativePath, TargetId, UnixTimestamp,
};

/// Specific mutation or maintenance operation type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationType {
    /// Delete a regular file or symlink
    DeleteFile {
        target_id: TargetId,
        rel_path: RelativePath,
        expected_identity: FileIdentity,
        estimated_size: ByteCount,
    },
    /// Remove an empty directory (after its children have been deleted)
    DeleteDirEmpty {
        target_id: TargetId,
        rel_path: RelativePath,
        expected_identity: FileIdentity,
    },
    /// Recursive prune of directory contents
    PruneDirRecursive {
        target_id: TargetId,
        rel_path: RelativePath,
        expected_identity: FileIdentity,
    },
    /// System maintenance: Filesystem TRIM / discard
    TrimFilesystem {
        mount_path: PathBuf,
    },
    /// System maintenance: Database VACUUM / defragmentation
    VacuumDatabase {
        db_path: PathBuf,
    },
}

impl OperationType {
    pub fn target_id(&self) -> Option<&TargetId> {
        match self {
            OperationType::DeleteFile { target_id, .. } => Some(target_id),
            OperationType::DeleteDirEmpty { target_id, .. } => Some(target_id),
            OperationType::PruneDirRecursive { target_id, .. } => Some(target_id),
            _ => None,
        }
    }

    pub fn is_dir(&self) -> bool {
        matches!(self, OperationType::DeleteDirEmpty { .. } | OperationType::PruneDirRecursive { .. })
    }
}

/// A planned operation within a deterministic operation graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedOperation {
    pub op_id: OperationId,
    pub op_type: OperationType,
    pub dependencies: Vec<OperationId>,
    pub estimated_reclaim: ByteCount,
}

/// Immutable, deterministic plan constructed by the Planner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedPlan {
    pub plan_id: PlanId,
    pub job_id: JobId,
    pub catalog_generation: CatalogGeneration,
    pub config_generation: ConfigGeneration,
    pub operations: Vec<PlannedOperation>,
    pub total_estimated_reclaim: ByteCount,
    pub created_at: UnixTimestamp,
}

impl PlannedPlan {
    pub fn empty(
        job_id: JobId,
        catalog_generation: CatalogGeneration,
        config_generation: ConfigGeneration,
    ) -> Self {
        Self {
            plan_id: PlanId(1),
            job_id,
            catalog_generation,
            config_generation,
            operations: Vec::new(),
            total_estimated_reclaim: ByteCount::ZERO,
            created_at: UnixTimestamp::now(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }
}
