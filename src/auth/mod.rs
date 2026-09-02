use std::sync::atomic::{AtomicU64, Ordering};

use crate::domain::grant::{AuthorizedPlan, Capability, CapabilityGrant};
use crate::domain::plan::{OperationType, PlannedPlan};
use crate::domain::types::{GenerationId, GrantId, UnixTimestamp};
use crate::error::{CleanerError, Result};

/// Authorization Engine issuing capability grants and enforcing generation binding.
#[derive(Debug)]
pub struct AuthorizationEngine {
    grant_id_counter: AtomicU64,
}

impl Default for AuthorizationEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthorizationEngine {
    pub fn new() -> Self {
        Self {
            grant_id_counter: AtomicU64::new(1),
        }
    }

    fn next_grant_id(&self) -> GrantId {
        GrantId(self.grant_id_counter.fetch_add(1, Ordering::Relaxed))
    }

    /// Evaluates a PlannedPlan and issues a generation-bound AuthorizedPlan with explicit capabilities.
    pub fn authorize_plan(
        &self,
        plan: PlannedPlan,
        current_catalog_gen: GenerationId,
        ttl_secs: u64,
        config_gen: GenerationId,
    ) -> Result<AuthorizedPlan> {
        // Invariant: Generation binding check
        if plan.catalog_generation != current_catalog_gen {
            return Err(CleanerError::SafetyViolation(format!(
                "Authorization rejected: plan generation {} is stale against catalog generation {}",
                plan.catalog_generation, current_catalog_gen
            )));
        }

        let mut capabilities = Vec::new();

        for op in &plan.operations {
            match &op.op_type {
                OperationType::DeleteFile {
                    target_id,
                    rel_path,
                    ..
                } => {
                    capabilities.push(Capability::DeleteFile(target_id.clone(), rel_path.clone()));
                }
                OperationType::DeleteDirEmpty {
                    target_id,
                    rel_path,
                    ..
                }
                | OperationType::PruneDirRecursive {
                    target_id,
                    rel_path,
                    ..
                } => {
                    capabilities.push(Capability::DeleteDirectory(
                        target_id.clone(),
                        rel_path.clone(),
                    ));
                }
                OperationType::TrimFilesystem { mount_path } => {
                    capabilities.push(Capability::TrimMount(mount_path.clone()));
                }
                OperationType::VacuumDatabase { .. } => {
                    // Handled as internal maintenance
                }
            }
        }

        let now = UnixTimestamp::now();
        let expires_at = UnixTimestamp(now.as_secs().saturating_add(ttl_secs));

        let grant = CapabilityGrant {
            grant_id: self.next_grant_id(),
            capabilities,
            catalog_generation: current_catalog_gen,
            config_generation: config_gen,
            granted_at: now,
            expires_at,
        };

        Ok(AuthorizedPlan { plan, grant })
    }
}
