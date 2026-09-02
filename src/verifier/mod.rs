use std::collections::HashMap;
use std::path::Path;
use serde::{Deserialize, Serialize};

use crate::catalog::CatalogSnapshot;
use crate::domain::plan::{OperationType, PlannedPlan};
use crate::domain::types::{FileIdentity, OperationId};
use crate::fs::SafeDirHandle;

/// Rich postcondition verification outcome classification for fine-grained correctness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VerificationOutcome {
    /// Object is confirmed absent on physical storage (Operation Succeeded).
    ConfirmedDeleted,
    /// Object was already absent (Idempotent Success).
    AlreadyGone,
    /// Object is still present on physical storage with matching identity (Operation Failed).
    StillPresent,
    /// An object exists at path, but identity (dev/ino) does not match (TOCTOU/Replaced).
    IdentityMismatch,
    /// Parent directory is unmounted, removed, or inaccessible.
    ParentUnavailable,
    /// Target descriptor is missing or unmounted.
    TargetUnavailable,
    /// Unknown storage state due to I/O or permissions error.
    Unknown,
}

impl VerificationOutcome {
    pub fn is_successful_deletion(&self) -> bool {
        matches!(self, Self::ConfirmedDeleted | Self::AlreadyGone)
    }
}

/// Postcondition Verifier.
/// Verifies physical storage truth following executor operations or during crash recovery.
#[derive(Debug, Default)]
pub struct PostconditionVerifier;

impl PostconditionVerifier {
    pub fn new() -> Self {
        Self
    }

    /// Verifies single operation postcondition returning granular verification outcome.
    pub fn verify_operation_postcondition(
        &self,
        base_path: &Path,
        rel_path: &Path,
        expected_identity: &FileIdentity,
    ) -> VerificationOutcome {
        let root_handle = match SafeDirHandle::open_root(base_path) {
            Ok(h) => h,
            Err(_) => return VerificationOutcome::TargetUnavailable,
        };

        let mut current_dir = root_handle;
        let components: Vec<_> = rel_path.components().collect();
        if components.is_empty() {
            return VerificationOutcome::Unknown;
        }

        for comp in &components[..components.len() - 1] {
            let name = match comp.as_os_str().to_str() {
                Some(n) => n,
                None => return VerificationOutcome::Unknown,
            };
            current_dir = match current_dir.open_child_dir(name) {
                Ok(h) => h,
                Err(_) => return VerificationOutcome::ConfirmedDeleted, // Parent dir gone -> item is deleted
            };
        }

        let leaf_name = match components.last().unwrap().as_os_str().to_str() {
            Some(n) => n,
            None => return VerificationOutcome::Unknown,
        };

        match current_dir.stat_child(leaf_name) {
            Ok(current_id) => {
                if current_id == *expected_identity {
                    VerificationOutcome::StillPresent
                } else {
                    VerificationOutcome::IdentityMismatch
                }
            }
            Err(_) => VerificationOutcome::ConfirmedDeleted,
        }
    }

    /// Verifies all operations in a plan, mapping each OperationId to its postcondition outcome.
    pub fn verify_plan_postcondition(
        &self,
        plan: &PlannedPlan,
        catalog: &CatalogSnapshot,
    ) -> HashMap<OperationId, VerificationOutcome> {
        let mut outcomes = HashMap::new();

        for op in &plan.operations {
            let (target_id, rel_path, expected_id) = match &op.op_type {
                OperationType::DeleteFile {
                    target_id,
                    rel_path,
                    expected_identity,
                    ..
                }
                | OperationType::DeleteDirEmpty {
                    target_id,
                    rel_path,
                    expected_identity,
                } => (target_id, rel_path.as_path(), expected_identity),
                _ => continue,
            };

            let outcome = match catalog.get(target_id) {
                Some(target) => {
                    self.verify_operation_postcondition(&target.base_path, rel_path, expected_id)
                }
                None => VerificationOutcome::TargetUnavailable,
            };

            outcomes.insert(op.op_id, outcome);
        }

        outcomes
    }
}
