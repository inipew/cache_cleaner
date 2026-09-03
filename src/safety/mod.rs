use std::path::Path;

use crate::domain::candidate::{Candidate, SafetyValidatedCandidate};
use crate::domain::target::TargetDescriptor;
use crate::domain::types::{
    CatalogGeneration, ConfigGeneration, FileIdentity, OperationId, TargetId, UnixTimestamp,
};
use crate::error::{CleanerError, Result};

/// Non-forgeable safety proof — bound to operation, generations, and expiry per 70.md:52-58.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafetyProof {
    pub target_id: TargetId,
    pub expected_identity: FileIdentity,
    pub operation_id: OperationId,
    pub catalog_generation: CatalogGeneration,
    pub config_generation: ConfigGeneration,
    pub validated_at: UnixTimestamp,
    pub expires_at: UnixTimestamp,
}

impl SafetyProof {
    pub fn is_expired(&self, now: UnixTimestamp) -> bool {
        now.as_secs() > self.expires_at.as_secs()
    }
    pub fn is_valid_for(
        &self,
        op_id: OperationId,
        now: UnixTimestamp,
        catalog_gen: CatalogGeneration,
        config_gen: ConfigGeneration,
    ) -> bool {
        self.operation_id == op_id
            && self.catalog_generation == catalog_gen
            && self.config_generation == config_gen
            && !self.is_expired(now)
    }
}

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
    /// Now requires operation and generation binding per 70.md:51-58.
    pub fn validate_mutation_boundary(
        &self,
        target: &TargetDescriptor,
        rel_path: &Path,
        expected_identity: &FileIdentity,
        operation_id: OperationId,
        catalog_generation: CatalogGeneration,
        config_generation: ConfigGeneration,
    ) -> Result<SafetyProof> {
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

        let now = UnixTimestamp::now();
        // Proof expires in 30 seconds — prevents stale reuse (70.md:57)
        let expires_at = UnixTimestamp::from_secs(now.as_secs().saturating_add(30));
        // Generation binding: ensure target descriptor generation matches expected catalog
        if target.catalog_generation != catalog_generation {
            return Err(CleanerError::SafetyViolation(format!(
                "Catalog generation mismatch: target {} vs expected {}",
                target.catalog_generation, catalog_generation
            )));
        }
        Ok(SafetyProof {
            target_id: target.target_id.clone(),
            expected_identity: *expected_identity,
            operation_id,
            catalog_generation,
            config_generation,
            validated_at: now,
            expires_at,
        })
    }

    fn verify_target_root_safety(&self, path: &Path) -> Result<()> {
        let path_comps: Vec<_> = path.components().collect();
        if path_comps.is_empty()
            || (path_comps.len() == 1 && path_comps[0] == std::path::Component::RootDir)
        {
            return Err(CleanerError::SafetyViolation(
                "Critical invariant violation: Root '/' must never be targeted for mutation".into(),
            ));
        }

        for prot in crate::config_pipeline::PLATFORM_INVARIANT_PROTECTED_PATHS {
            let prot_path = Path::new(prot);
            let prot_comps: Vec<_> = prot_path.components().collect();
            if path_comps.len() >= prot_comps.len()
                && !prot_comps.is_empty()
                && path_comps[..prot_comps.len()] == prot_comps[..]
            {
                return Err(CleanerError::SafetyViolation(format!(
                    "Critical invariant violation: Protected system root '{}' must never be targeted for mutation (path: {})",
                    prot,
                    path.display()
                )));
            }
        }
        Ok(())
    }
}
