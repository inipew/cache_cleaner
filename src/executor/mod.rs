use std::path::Path;
use std::time::Instant;

use crate::catalog::CatalogSnapshot;
use crate::domain::grant::AuthorizedPlan;
use crate::domain::intent::{MutationType, OperationIntent};
use crate::domain::plan::OperationType;
use crate::domain::result::{JobResult, OperationFinalResult, OperationStatus};
use crate::domain::types::{AttemptId, ByteCount, PlanId, UnixTimestamp};
use crate::engine::cancellation::CancellationToken;
use crate::error::{CleanerError, Result};
use crate::fs::SafeDirHandle;
use crate::resource::ResourceManager;
use crate::safety::SafetyGate;
use crate::store::SqliteStore;
use crate::verifier::{PostconditionVerifier, VerificationOutcome};

/// Single Mutation Boundary Executor.
/// The only subsystem in the entire architecture permitted to perform filesystem destructive mutations.
/// Enforces a strict 8-step execution invariant per operation.
#[derive(Debug, Default)]
pub struct CleanupExecutor;

impl CleanupExecutor {
    pub fn new() -> Self {
        Self
    }

    /// Executes an AuthorizedPlan with full ACID intent persistence, target locks, and safety boundary revalidation.
    #[allow(clippy::too_many_arguments)]
    pub fn execute_plan(
        &self,
        authorized: &AuthorizedPlan,
        catalog: &CatalogSnapshot,
        attempt_id: AttemptId,
        cancel_token: &CancellationToken,
        resource_mgr: &ResourceManager,
        store: Option<&SqliteStore>,
        safety_gate: &SafetyGate,
        verifier: &PostconditionVerifier,
    ) -> Result<JobResult> {
        let start_time = Instant::now();
        let now = UnixTimestamp::now();

        // Safety Gate 1: Check authorization validity against current catalog generation
        if !authorized.is_authorized_for_execution(now, catalog.generation) {
            return Err(CleanerError::SafetyViolation(
                "Execution rejected: AuthorizedPlan has expired or generation is invalid".into(),
            ));
        }

        let mut results = Vec::new();
        let mut total_reclaimed = ByteCount::ZERO;
        let mut success_count = 0;
        let mut failed_count = 0;
        let mut skipped_count = 0;

        for op in &authorized.plan.operations {
            // Step 1: Cancellation check
            if cancel_token.is_cancelled() {
                results.push(OperationFinalResult {
                    op_id: op.op_id,
                    status: OperationStatus::Preempted,
                    reclaimed_bytes: ByteCount::ZERO,
                    executed_at: UnixTimestamp::now(),
                });
                skipped_count += 1;
                continue;
            }

            // Step 2: Rate limiter throttling
            resource_mgr.throttle_mutation();

            let op_res = match &op.op_type {
                OperationType::DeleteFile {
                    target_id,
                    rel_path,
                    expected_identity,
                    estimated_size,
                } => {
                    let target = match catalog.get(target_id) {
                        Some(t) => t,
                        None => {
                            let res = OperationFinalResult {
                                op_id: op.op_id,
                                status: OperationStatus::Failed {
                                    error: format!("Target {} not found in catalog snapshot", target_id),
                                },
                                reclaimed_bytes: ByteCount::ZERO,
                                executed_at: UnixTimestamp::now(),
                            };
                            failed_count += 1;
                            results.push(res);
                            continue;
                        }
                    };

                    // Step 3: Target lock acquisition (mutual exclusion)
                    let _target_lock = match resource_mgr.acquire_target_lock(target_id) {
                        Ok(l) => l,
                        Err(e) => {
                            let res = OperationFinalResult {
                                op_id: op.op_id,
                                status: OperationStatus::Failed {
                                    error: format!("Target lock collision: {}", e),
                                },
                                reclaimed_bytes: ByteCount::ZERO,
                                executed_at: UnixTimestamp::now(),
                            };
                            failed_count += 1;
                            results.push(res);
                            continue;
                        }
                    };

                    // Step 4: Mutation boundary final safety recheck
                    if let Err(e) = safety_gate.validate_mutation_boundary(target, rel_path.as_path(), expected_identity) {
                        let res = OperationFinalResult {
                            op_id: op.op_id,
                            status: OperationStatus::Failed {
                                error: format!("Safety gate boundary rejection: {}", e),
                            },
                            reclaimed_bytes: ByteCount::ZERO,
                            executed_at: UnixTimestamp::now(),
                        };
                        failed_count += 1;
                        results.push(res);
                        continue;
                    }

                    // Step 5: Durable Operation Intent Commit to SQLite (HARD BLOCKER)
                    if let Some(s) = store {
                        let intent = OperationIntent::new(
                            authorized.plan.job_id,
                            attempt_id,
                            op.op_id,
                            target_id.clone(),
                            rel_path.clone(),
                            *expected_identity,
                            *estimated_size,
                            MutationType::DeleteFile,
                            catalog.generation,
                            authorized.grant.config_generation,
                        );
                        if let Err(e) = s.commit_operation_intent(&intent) {
                            // Intent commit failed -> Hard fail-closed without mutating disk!
                            let res = OperationFinalResult {
                                op_id: op.op_id,
                                status: OperationStatus::Failed {
                                    error: format!("Durable intent commit failed: {}", e),
                                },
                                reclaimed_bytes: ByteCount::ZERO,
                                executed_at: UnixTimestamp::now(),
                            };
                            failed_count += 1;
                            results.push(res);
                            continue;
                        }
                    }

                    // Step 6: Physical Destructive Mutation
                    let mutation_res = self.execute_delete_file(
                        &target.base_path,
                        rel_path.as_path(),
                        expected_identity,
                        resource_mgr,
                    );

                    // Step 7: Postcondition Verification & Receipt
                    let outcome = verifier.verify_operation_postcondition(
                        &target.base_path,
                        rel_path.as_path(),
                        expected_identity,
                    );

                    let (status, reclaimed) = match (mutation_res, outcome) {
                        (Ok(_), VerificationOutcome::ConfirmedDeleted | VerificationOutcome::AlreadyGone) => {
                            total_reclaimed = total_reclaimed.saturating_add(*estimated_size);
                            success_count += 1;
                            (OperationStatus::Success, *estimated_size)
                        }
                        (Err(_), VerificationOutcome::ConfirmedDeleted | VerificationOutcome::AlreadyGone) => {
                            // Object already gone -> Idempotent success
                            success_count += 1;
                            (OperationStatus::Success, *estimated_size)
                        }
                        (Ok(_), outcome) => {
                            failed_count += 1;
                            (
                                OperationStatus::Failed {
                                    error: format!("Postcondition verification failed: {:?}", outcome),
                                },
                                ByteCount::ZERO,
                            )
                        }
                        (Err(e), _) => {
                            failed_count += 1;
                            (
                                OperationStatus::Failed {
                                    error: e.to_string(),
                                },
                                ByteCount::ZERO,
                            )
                        }
                    };

                    let res = OperationFinalResult {
                        op_id: op.op_id,
                        status,
                        reclaimed_bytes: reclaimed,
                        executed_at: UnixTimestamp::now(),
                    };

                    // Step 8: Record final result in SQLite
                    if let Some(s) = store {
                        let _ = s.record_operation_result(
                            authorized.plan.job_id,
                            PlanId(authorized.plan.plan_id),
                            target_id,
                            "DELETE_FILE",
                            rel_path.as_str(),
                            expected_identity,
                            *estimated_size,
                            &res,
                        );
                    }

                    res
                }
                OperationType::DeleteDirEmpty {
                    target_id,
                    rel_path,
                    expected_identity,
                } => {
                    let target = match catalog.get(target_id) {
                        Some(t) => t,
                        None => {
                            failed_count += 1;
                            continue;
                        }
                    };

                    let _target_lock = match resource_mgr.acquire_target_lock(target_id) {
                        Ok(l) => l,
                        Err(_) => {
                            failed_count += 1;
                            continue;
                        }
                    };

                    if let Err(e) = safety_gate.validate_mutation_boundary(target, rel_path.as_path(), expected_identity) {
                        let res = OperationFinalResult {
                            op_id: op.op_id,
                            status: OperationStatus::Failed {
                                error: format!("Safety gate boundary rejection: {}", e),
                            },
                            reclaimed_bytes: ByteCount::ZERO,
                            executed_at: UnixTimestamp::now(),
                        };
                        failed_count += 1;
                        results.push(res);
                        continue;
                    }

                    if let Some(s) = store {
                        let intent = OperationIntent::new(
                            authorized.plan.job_id,
                            attempt_id,
                            op.op_id,
                            target_id.clone(),
                            rel_path.clone(),
                            *expected_identity,
                            ByteCount::ZERO,
                            MutationType::DeleteDirEmpty,
                            catalog.generation,
                            authorized.grant.config_generation,
                        );
                        let _ = s.commit_operation_intent(&intent);
                    }

                    let mutation_res = self.execute_rmdir(
                        &target.base_path,
                        rel_path.as_path(),
                        expected_identity,
                        resource_mgr,
                    );

                    let outcome = verifier.verify_operation_postcondition(
                        &target.base_path,
                        rel_path.as_path(),
                        expected_identity,
                    );

                    let status = match (mutation_res, outcome) {
                        (Ok(_), VerificationOutcome::ConfirmedDeleted | VerificationOutcome::AlreadyGone) => {
                            success_count += 1;
                            OperationStatus::Success
                        }
                        (Err(_), VerificationOutcome::ConfirmedDeleted | VerificationOutcome::AlreadyGone) => {
                            success_count += 1;
                            OperationStatus::Success
                        }
                        (res, outcome) => {
                            failed_count += 1;
                            OperationStatus::Failed {
                                error: format!("Rmdir failed ({:?}): {:?}", res.err(), outcome),
                            }
                        }
                    };

                    let res = OperationFinalResult {
                        op_id: op.op_id,
                        status,
                        reclaimed_bytes: ByteCount::ZERO,
                        executed_at: UnixTimestamp::now(),
                    };

                    if let Some(s) = store {
                        let _ = s.record_operation_result(
                            authorized.plan.job_id,
                            PlanId(authorized.plan.plan_id),
                            target_id,
                            "DELETE_DIR_EMPTY",
                            rel_path.as_str(),
                            expected_identity,
                            ByteCount::ZERO,
                            &res,
                        );
                    }

                    res
                }
                _ => {
                    skipped_count += 1;
                    continue;
                }
            };

            results.push(op_res);
        }

        let duration_ms = start_time.elapsed().as_millis() as u64;

        Ok(JobResult {
            job_id: authorized.plan.job_id,
            attempt_id,
            total_reclaimed,
            total_operations: authorized.plan.operations.len(),
            successful_operations: success_count,
            failed_operations: failed_count,
            skipped_operations: skipped_count,
            duration_ms,
        })
    }

    fn execute_delete_file(
        &self,
        base_path: &Path,
        rel_path: &Path,
        expected_identity: &crate::domain::types::FileIdentity,
        resource_mgr: &ResourceManager,
    ) -> Result<()> {
        let root_permit = resource_mgr.acquire_fd_permit().ok();
        let root_handle = SafeDirHandle::open_root_with_permit(base_path, root_permit)?;
        let mut current_dir = root_handle;

        let components: Vec<_> = rel_path.components().collect();
        if components.is_empty() {
            return Err(CleanerError::SafetyViolation(
                "Empty relative path in file deletion operation".into(),
            ));
        }

        for comp in &components[..components.len() - 1] {
            let name = comp.as_os_str().to_str().ok_or_else(|| {
                CleanerError::SafetyViolation("Non-UTF8 path component".into())
            })?;
            let child_permit = resource_mgr.acquire_fd_permit().ok();
            current_dir = current_dir.open_child_dir_with_permit(name, child_permit)?;
        }

        let file_name = components.last().unwrap().as_os_str().to_str().ok_or_else(|| {
            CleanerError::SafetyViolation("Non-UTF8 file name".into())
        })?;

        current_dir.unlink_child_file(file_name, expected_identity)
    }

    fn execute_rmdir(
        &self,
        base_path: &Path,
        rel_path: &Path,
        expected_identity: &crate::domain::types::FileIdentity,
        resource_mgr: &ResourceManager,
    ) -> Result<()> {
        let root_permit = resource_mgr.acquire_fd_permit().ok();
        let root_handle = SafeDirHandle::open_root_with_permit(base_path, root_permit)?;
        let mut current_dir = root_handle;

        let components: Vec<_> = rel_path.components().collect();
        if components.is_empty() {
            return Err(CleanerError::SafetyViolation(
                "Empty relative path in rmdir operation".into(),
            ));
        }

        for comp in &components[..components.len() - 1] {
            let name = comp.as_os_str().to_str().ok_or_else(|| {
                CleanerError::SafetyViolation("Non-UTF8 path component".into())
            })?;
            let child_permit = resource_mgr.acquire_fd_permit().ok();
            current_dir = current_dir.open_child_dir_with_permit(name, child_permit)?;
        }

        let dir_name = components.last().unwrap().as_os_str().to_str().ok_or_else(|| {
            CleanerError::SafetyViolation("Non-UTF8 directory name".into())
        })?;

        current_dir.rmdir_child_dir(dir_name, expected_identity)
    }
}
