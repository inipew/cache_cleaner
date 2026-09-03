use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::domain::plan::PlannedPlan;
use crate::domain::types::{
    CatalogGeneration, ConfigGeneration, GrantId, RelativePath, TargetId, UnixTimestamp,
};

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
    /// Permission to vacuum and defragment database
    VacuumDatabase(PathBuf),
}

/// Explicit, scoped capability grant issued for an authorized plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityGrant {
    pub grant_id: GrantId,
    pub capabilities: Vec<Capability>,
    pub catalog_generation: CatalogGeneration,
    pub config_generation: ConfigGeneration,
    pub granted_at: UnixTimestamp,
    pub expires_at: UnixTimestamp,
}

impl CapabilityGrant {
    pub fn is_valid_at(
        &self,
        now: UnixTimestamp,
        expected_cat_gen: CatalogGeneration,
        expected_cfg_gen: ConfigGeneration,
    ) -> bool {
        self.catalog_generation == expected_cat_gen
            && self.config_generation == expected_cfg_gen
            && now.as_secs() <= self.expires_at.as_secs()
    }
}

/// Immutable, fully authorized plan ready for execution by the Executor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorizedPlan {
    pub plan: PlannedPlan,
    pub grant: CapabilityGrant,
}

impl AuthorizedPlan {
    pub fn is_authorized_for_execution(
        &self,
        now: UnixTimestamp,
        current_cat_gen: CatalogGeneration,
        current_cfg_gen: ConfigGeneration,
    ) -> bool {
        // Check BOTH generations — previous bug used grant's own config gen, always true for config
        if self.plan.config_generation != current_cfg_gen {
            return false;
        }
        self.grant.is_valid_at(now, current_cat_gen, current_cfg_gen)
            && self.plan.catalog_generation == current_cat_gen
    }
}
