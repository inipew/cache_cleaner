use std::collections::HashMap;

use crate::domain::decision::PolicyPermit;
use crate::domain::plan::{OperationType, PlannedOperation, PlannedPlan};
use crate::domain::types::{ByteCount, GenerationId, JobId, OperationId, PlanId, UnixTimestamp};

/// Deterministic Planner constructing a canonical hierarchical operation DAG.
#[derive(Debug, Default)]
pub struct CleanupPlanner;

impl CleanupPlanner {
    pub fn new() -> Self {
        Self
    }

    /// Builds a deterministic, immutable PlannedPlan with explicit child-to-parent dependencies.
    pub fn build_plan(
        &self,
        job_id: JobId,
        catalog_gen: GenerationId,
        permits: Vec<PolicyPermit>,
    ) -> PlannedPlan {
        let plan_id = PlanId((job_id.0 << 16) ^ (catalog_gen.0 << 8) ^ 1);
        let mut operations = Vec::new();
        let mut total_reclaim = ByteCount::ZERO;
        let mut op_id_counter = 1u64;

        // Separate files and directories
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

        let mut path_to_op_id: HashMap<String, OperationId> = HashMap::new();

        // 1. Plan files (leaf nodes with zero incoming dependencies)
        for permit in files {
            let cand = permit.candidate.candidate;
            let op_id = OperationId(op_id_counter);
            op_id_counter += 1;

            total_reclaim = total_reclaim.saturating_add(cand.size_bytes);
            path_to_op_id.insert(cand.rel_path.as_str().to_string(), op_id);

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

        // 2. Plan directories with explicit dependencies on child files/dirs
        for permit in dirs {
            let cand = permit.candidate.candidate;
            let op_id = OperationId(op_id_counter);
            op_id_counter += 1;

            let dir_prefix = format!("{}/", cand.rel_path.as_str());

            // Collect all dependencies (operations within this directory that must complete before rmdir)
            let mut dependencies = Vec::new();
            for (path, child_op_id) in &path_to_op_id {
                if path.starts_with(&dir_prefix) {
                    dependencies.push(*child_op_id);
                }
            }
            dependencies.sort();

            path_to_op_id.insert(cand.rel_path.as_str().to_string(), op_id);

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

        PlannedPlan {
            plan_id: plan_id.0,
            job_id,
            catalog_generation: catalog_gen,
            operations,
            total_estimated_reclaim: total_reclaim,
            created_at: UnixTimestamp::now(),
        }
    }
}
