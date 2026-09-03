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
    ConfirmedDeleted,
    AlreadyGone,
    StillPresent,
    IdentityMismatch,
    ParentUnavailable,
    TargetUnavailable,
    Stale,
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
            current_dir = match current_dir.open_child_dir_errno(name) {
                Ok(h) => h,
                Err(rustix::io::Errno::NOENT) => return VerificationOutcome::AlreadyGone,
                Err(rustix::io::Errno::ACCESS) | Err(rustix::io::Errno::PERM) => {
                    return VerificationOutcome::ParentUnavailable;
                }
                Err(rustix::io::Errno::IO) | Err(rustix::io::Errno::STALE) => {
                    return VerificationOutcome::Unknown;
                }
                Err(rustix::io::Errno::BUSY) | Err(rustix::io::Errno::ROFS) => return VerificationOutcome::TargetUnavailable,
                Err(_) => return VerificationOutcome::ParentUnavailable,
            };
        }

        let leaf_name = match components.last().unwrap().as_os_str().to_str() {
            Some(n) => n,
            None => return VerificationOutcome::Unknown,
        };

        self.verify_with_parent_handle(&current_dir, leaf_name, expected_identity)
    }

    /// Verifies postcondition using an active parent directory handle directly.
    pub fn verify_with_parent_handle(
        &self,
        parent_dir: &SafeDirHandle,
        leaf_name: &str,
        expected_identity: &FileIdentity,
    ) -> VerificationOutcome {
        match parent_dir.stat_child_errno(leaf_name) {
            Ok(current_id) => {
                if current_id == *expected_identity {
                    VerificationOutcome::StillPresent
                } else {
                    VerificationOutcome::IdentityMismatch
                }
            }
            Err(rustix::io::Errno::NOENT) => VerificationOutcome::ConfirmedDeleted,
            Err(rustix::io::Errno::ROFS) | Err(rustix::io::Errno::BUSY) => VerificationOutcome::TargetUnavailable,
            Err(rustix::io::Errno::NOTDIR) | Err(rustix::io::Errno::STALE) => VerificationOutcome::ParentUnavailable,
            Err(rustix::io::Errno::ACCESS) | Err(rustix::io::Errno::PERM) => VerificationOutcome::Unknown,
            Err(rustix::io::Errno::IO) => VerificationOutcome::Unknown,
            Err(_) => VerificationOutcome::Unknown,
        }
    }

    /// Verifies all operations in a plan, mapping each OperationId to its postcondition outcome.
    /// Generation stale check per 43.md:22 — if plan generation mismatches catalog, return Stale for all.
    pub fn verify_plan_postcondition(
        &self,
        plan: &PlannedPlan,
        catalog: &CatalogSnapshot,
    ) -> HashMap<OperationId, VerificationOutcome> {
        if plan.catalog_generation != catalog.generation {
            let mut outcomes = HashMap::new();
            for op in &plan.operations {
                outcomes.insert(op.op_id, VerificationOutcome::Stale);
            }
            return outcomes;
        }
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
