use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::domain::plan::PlannedPlan;
use crate::domain::types::{GenerationId, GrantId, RelativePath, TargetId, UnixTimestamp};

/// Explicit capabilities granted by the Authorization Engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Capability {
    /// Permission to read files in target
    ReadTarget(TargetId),
    /// Permission to enumerate directory entries in target
    EnumerateTarget(TargetId),
    /// Permission to delete specific file in target
    DeleteFile(TargetId, RelativePath),
    /// Permission to remove directory in target
    DeleteDirectory(TargetId, RelativePath),
    /// Permission to trigger TRIM on a filesystem mount
    TrimMount(PathBuf),
}

/// Explicit, scoped capability grant issued for an authorized plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityGrant {
    pub grant_id: GrantId,
    pub capabilities: Vec<Capability>,
    pub catalog_generation: GenerationId,
    pub config_generation: GenerationId,
    pub granted_at: UnixTimestamp,
    pub expires_at: UnixTimestamp,
}

impl CapabilityGrant {
    pub fn is_valid_at(&self, now: UnixTimestamp, expected_gen: GenerationId) -> bool {
        self.catalog_generation == expected_gen && now.as_secs() <= self.expires_at.as_secs()
    }
}

/// Immutable, fully authorized plan ready for execution by the Executor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorizedPlan {
    pub plan: PlannedPlan,
    pub grant: CapabilityGrant,
}

impl AuthorizedPlan {
    pub fn is_authorized_for_execution(&self, now: UnixTimestamp, current_gen: GenerationId) -> bool {
        self.grant.is_valid_at(now, current_gen) && self.plan.catalog_generation == current_gen
    }
}
