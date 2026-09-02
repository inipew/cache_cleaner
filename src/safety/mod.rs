use std::path::Path;

use crate::domain::candidate::{Candidate, SafetyValidatedCandidate};
use crate::domain::target::TargetDescriptor;
use crate::domain::types::FileIdentity;
use crate::error::{CleanerError, Result};

/// Safety Gate engine enforcing absolute filesystem invariants before policy evaluation and at the mutation boundary.
#[derive(Debug, Default)]
pub struct SafetyGate;

impl SafetyGate {
    pub fn new() -> Self {
        Self
    }

    /// Evaluates a candidate against physical filesystem invariants and target safety tiers.
    pub fn validate_candidate(
        &self,
        candidate: Candidate,
        target: &TargetDescriptor,
    ) -> Result<SafetyValidatedCandidate> {
        // Invariant 1: Target must allow mutation
        if !target.is_mutation_allowed() {
            return Err(CleanerError::SafetyViolation(format!(
                "Target {} safety tier {:?} strictly forbids mutation",
                target.target_id, target.safety_tier
            )));
        }

        // Invariant 2: Candidate target ID must match descriptor
        if candidate.target_id != target.target_id {
            return Err(CleanerError::SafetyViolation(format!(
                "Candidate target ID mismatch: candidate {} vs target {}",
                candidate.target_id, target.target_id
            )));
        }

        // Invariant 3: Device must match the target's trusted root device (no mount crossing)
        if candidate.identity.dev != target.dev {
            return Err(CleanerError::SafetyViolation(format!(
                "Mount boundary violation: candidate dev {} != target root dev {}",
                candidate.identity.dev, target.dev
            )));
        }

        // Invariant 4: Prevent deletion of target base directory itself
        if candidate.rel_path.as_path().as_os_str().is_empty() {
            return Err(CleanerError::SafetyViolation(
                "Attempted to validate root target directory itself as a deletion candidate".into(),
            ));
        }

        // Invariant 5: Protected root path verification
        self.verify_target_root_safety(&target.base_path)?;

        Ok(SafetyValidatedCandidate::new(
            candidate,
            target.dev,
            target.ino,
        ))
    }

    /// Final safety revalidation performed right before physical filesystem destructive mutation.
    pub fn validate_mutation_boundary(
        &self,
        target: &TargetDescriptor,
        rel_path: &Path,
        expected_identity: &FileIdentity,
    ) -> Result<()> {
        if !target.is_mutation_allowed() {
            return Err(CleanerError::SafetyViolation(format!(
                "Mutation boundary rejected: Target {} safety tier {:?} is read-only",
                target.target_id, target.safety_tier
            )));
        }

        self.verify_target_root_safety(&target.base_path)?;

        if rel_path.as_os_str().is_empty() {
            return Err(CleanerError::SafetyViolation(
                "Mutation boundary rejected: Attempted to mutate target root directory directly".into(),
            ));
        }

        if expected_identity.dev != target.dev {
            return Err(CleanerError::SafetyViolation(format!(
                "Mutation boundary rejected: Device mismatch expected {} vs target root {}",
                expected_identity.dev, target.dev
            )));
        }

        Ok(())
    }

    fn verify_target_root_safety(&self, path: &Path) -> Result<()> {
        let p_str = path.to_string_lossy();
        if p_str == "/" || p_str == "/system" || p_str == "/vendor" || p_str == "/etc" || p_str == "/boot" {
            return Err(CleanerError::SafetyViolation(format!(
                "Critical invariant violation: Protected system root '{}' must never be targeted for mutation",
                p_str
            )));
        }
        Ok(())
    }
}
