use std::collections::{HashMap, HashSet};

use crate::domain::decision::PolicyPermit;
use crate::domain::plan::{OperationType, PlannedOperation, PlannedPlan};
use crate::domain::types::{ByteCount, CatalogGeneration, ConfigGeneration, JobId, OperationId, PlanId, TargetId, UnixTimestamp};
use crate::error::{CleanerError, Result};

pub const DEFAULT_MAX_OPERATIONS_PER_PLAN: usize = 10_000;

/// Deterministic Planner constructing a canonical hierarchical operation DAG.
#[derive(Debug, Default)]
pub struct CleanupPlanner;

impl CleanupPlanner {
    pub fn new() -> Self {
        Self
    }

    /// Builds a deterministic, immutable PlannedPlan with explicit child-to-parent dependencies and target scoping.
    pub fn build_plan(
        &self,
        job_id: JobId,
        catalog_gen: CatalogGeneration,
        config_gen: ConfigGeneration,
        permits: Vec<PolicyPermit>,
    ) -> Result<PlannedPlan> {
        // P1 Fix: Strictly enforce budget - fail closed instead of silent truncation (Spec 72.md)
        if permits.len() > DEFAULT_MAX_OPERATIONS_PER_PLAN {
            return Err(CleanerError::PlanBudgetExceeded {
                count: permits.len(),
                limit: DEFAULT_MAX_OPERATIONS_PER_PLAN,
            });
        }

        let plan_id = PlanId((job_id.0 << 16) ^ (catalog_gen.0 << 8) ^ 1);
        let mut operations = Vec::new();
        let mut total_reclaim = ByteCount::ZERO;
        let mut op_id_counter = 1u64;

        // Separate files and directories within bounded plan budget (Dokumen 40, 72)
        let mut files = Vec::new();
        let mut dirs = Vec::new();

        for permit in permits {
            if permit.candidate.candidate.is_dir {
                dirs.push(permit);
            } else {
                files.push(permit);
            }
        }

        // Canonical sort for files: target_id -> depth descending -> path bytes
        files.sort_by(|a, b| {
            let ca = &a.candidate.candidate;
            let cb = &b.candidate.candidate;
            let depth_a = ca.rel_path.as_str().split('/').count();
            let depth_b = cb.rel_path.as_str().split('/').count();

            ca.target_id
                .0
                .cmp(&cb.target_id.0)
                .then(depth_b.cmp(&depth_a)) // Deepest first
                .then(ca.rel_path.as_str().cmp(cb.rel_path.as_str()))
        });

        // Canonical sort for directories: target_id -> depth descending -> path bytes
        dirs.sort_by(|a, b| {
            let ca = &a.candidate.candidate;
            let cb = &b.candidate.candidate;
            let depth_a = ca.rel_path.as_str().split('/').count();
            let depth_b = cb.rel_path.as_str().split('/').count();

            ca.target_id
                .0
                .cmp(&cb.target_id.0)
                .then(depth_b.cmp(&depth_a)) // Deepest directory first
                .then(ca.rel_path.as_str().cmp(cb.rel_path.as_str()))
        });

        // Scoped key: (TargetId, String) to prevent collision between targets with identical subpaths
        let mut path_to_op_id: HashMap<(TargetId, String), OperationId> = HashMap::new();
        let mut defined_ops: HashSet<OperationId> = HashSet::new();

        // 1. Plan files (leaf nodes with zero incoming dependencies)
        for permit in files {
            let cand = permit.candidate.candidate;
            let op_id = OperationId(op_id_counter);
            op_id_counter += 1;

            total_reclaim = total_reclaim.saturating_add(cand.size_bytes);
            path_to_op_id.insert((cand.target_id.clone(), cand.rel_path.as_str().to_string()), op_id);
            defined_ops.insert(op_id);

            operations.push(PlannedOperation {
                op_id,
                op_type: OperationType::DeleteFile {
                    target_id: cand.target_id,
                    rel_path: cand.rel_path,
                    expected_identity: cand.identity,
                    estimated_size: cand.size_bytes,
                },
                dependencies: Vec::new(),
                estimated_reclaim: cand.size_bytes,
            });
        }

        // 2. Plan directories with explicit dependencies on child files/dirs strictly within the same target
        for permit in dirs {
            let cand = permit.candidate.candidate;
            let op_id = OperationId(op_id_counter);
            op_id_counter += 1;

            let dir_prefix = format!("{}/", cand.rel_path.as_str());

            // Collect all dependencies (operations strictly within this target and directory that must complete before rmdir)
            let mut dependencies = Vec::new();
            for ((t_id, path), child_op_id) in &path_to_op_id {
                if *t_id == cand.target_id && path.starts_with(&dir_prefix) {
                    dependencies.push(*child_op_id);
                }
            }
            dependencies.sort();
            dependencies.dedup();

            // Sanity check: Ensure all dependencies are valid and strictly precede this operation (acyclic DAG)
            dependencies.retain(|dep| defined_ops.contains(dep));

            path_to_op_id.insert((cand.target_id.clone(), cand.rel_path.as_str().to_string()), op_id);
            defined_ops.insert(op_id);

            operations.push(PlannedOperation {
                op_id,
                op_type: OperationType::DeleteDirEmpty {
                    target_id: cand.target_id,
                    rel_path: cand.rel_path,
                    expected_identity: cand.identity,
                },
                dependencies,
                estimated_reclaim: ByteCount::ZERO,
            });
        }

        let plan = PlannedPlan {
            plan_id,
            job_id,
            catalog_generation: catalog_gen,
            config_generation: config_gen,
            operations,
            total_estimated_reclaim: total_reclaim,
            created_at: UnixTimestamp::now(),
        };

        self.validate_plan(&plan)?;
        Ok(plan)
    }

    /// Validates acyclic DAG dependencies, uniqueness, and operation bounds.
    pub fn validate_plan(&self, plan: &PlannedPlan) -> Result<()> {
        if plan.operations.len() > DEFAULT_MAX_OPERATIONS_PER_PLAN {
            return Err(CleanerError::PlanBudgetExceeded {
                count: plan.operations.len(),
                limit: DEFAULT_MAX_OPERATIONS_PER_PLAN,
            });
        }

        let mut seen_ops = HashSet::with_capacity(plan.operations.len());
        for op in &plan.operations {
            if !seen_ops.insert(op.op_id) {
                return Err(CleanerError::PlanValidationFailed(format!(
                    "Duplicate OperationId {} detected in plan",
                    op.op_id
                )));
            }
            for dep in &op.dependencies {
                if !seen_ops.contains(dep) {
                    return Err(CleanerError::PlanValidationFailed(format!(
                        "Invalid DAG dependency: Operation {} depends on unfulfilled or succeeding operation {}",
                        op.op_id, dep
                    )));
                }
            }
        }

        Ok(())
    }
}
