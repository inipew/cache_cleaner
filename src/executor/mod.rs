use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::catalog::CatalogSnapshot;
use crate::domain::grant::{AuthorizedPlan, Capability};
use crate::domain::intent::{MutationType, OperationIntent};
use crate::domain::plan::OperationType;
use crate::domain::result::{JobResult, OperationFinalResult, OperationStatus};
use crate::domain::target::TargetDescriptor;
use crate::domain::types::{AttemptId, ByteCount, ConfigGeneration, OperationId, TargetId, UnixTimestamp};
use crate::engine::cancellation::CancellationToken;
use crate::error::{CleanerError, Result};
use crate::fs::SafeDirHandle;
use crate::resource::ResourceManager;
use crate::safety::SafetyGate;
use crate::store::SqliteStore;
use crate::verifier::{PostconditionVerifier, VerificationOutcome};

/// Cached descriptor for an active parent directory to avoid redundant openat iterations from target root.
struct ActiveDirCache {
    target_id: TargetId,
    parent_path: PathBuf,
    handle: SafeDirHandle,
}

/// Single Mutation Boundary Executor.
/// The only subsystem in the entire architecture permitted to perform filesystem destructive mutations.
/// Enforces a strict 8-step execution invariant per operation with DAG dependency and CapabilityGrant enforcement.
#[derive(Debug, Default)]
pub struct CleanupExecutor;

impl CleanupExecutor {
    pub fn new() -> Self {
        Self
    }

    /// Executes an AuthorizedPlan with full ACID intent persistence, target locks, capability enforcement, and safety boundary revalidation.
    #[allow(clippy::too_many_arguments)]
    pub fn execute_plan(
        &self,
        authorized: &AuthorizedPlan,
        catalog: &CatalogSnapshot,
        current_config_gen: ConfigGeneration,
        attempt_id: AttemptId,
        cancel_token: &CancellationToken,
        resource_mgr: &ResourceManager,
        store: &SqliteStore,
        safety_gate: &SafetyGate,
        verifier: &PostconditionVerifier,
    ) -> Result<JobResult> {
        let start_time = Instant::now();
        let now = UnixTimestamp::now();

        // Safety Gate 1: Check authorization validity against current catalog & config generations (E1 Fix: use live current_config_gen)
        if !authorized.is_authorized_for_execution(now, catalog.generation, current_config_gen) {
            return Err(CleanerError::SafetyViolation(
                "Execution rejected: AuthorizedPlan has expired or generation is invalid".into(),
            ));
        }

        let mut results = Vec::new();
        let mut total_reclaimed = ByteCount::ZERO;
        let mut success_count = 0;
        let mut failed_count = 0;
        let mut skipped_count = 0;
        let mut successful_op_ids: HashSet<OperationId> = HashSet::new();

        // Per-op intent commitment after all gates succeed (C6 fix: no bulk before validation).
        // Intents are committed only for operations that pass DAG/capability/lock/safety.
        let mut active_dir_cache: Option<ActiveDirCache> = None;

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

            // Step 2: DAG Dependency Verification (Dependencies MUST have completed successfully)
            let dependencies_unfulfilled = op.dependencies.iter().any(|dep| !successful_op_ids.contains(dep));
            if dependencies_unfulfilled {
                let res = OperationFinalResult {
                    op_id: op.op_id,
                    status: OperationStatus::Skipped {
                        reason: "Precondition dependency failed or unfulfilled".into(),
                    },
                    reclaimed_bytes: ByteCount::ZERO,
                    executed_at: UnixTimestamp::now(),
                };
                skipped_count += 1;
                results.push(res);
                continue;
            }

            // Step 3: Capability Grant Enforcement Gate
            let has_capability = match &op.op_type {
                OperationType::DeleteFile { target_id, rel_path, .. } => {
                    authorized.grant.capabilities.contains(&Capability::DeleteFile(target_id.clone(), rel_path.clone()))
                }
                OperationType::DeleteDirEmpty { target_id, rel_path, .. }
                | OperationType::PruneDirRecursive { target_id, rel_path, .. } => {
                    authorized.grant.capabilities.contains(&Capability::DeleteDirectory(target_id.clone(), rel_path.clone()))
                }
                OperationType::TrimFilesystem { mount_path } => {
                    authorized.grant.capabilities.contains(&Capability::TrimMount(mount_path.clone()))
                }
                OperationType::VacuumDatabase { db_path } => {
                    authorized.grant.capabilities.contains(&Capability::VacuumDatabase(db_path.clone()))
                }
            };

            if !has_capability {
                let res = OperationFinalResult {
                    op_id: op.op_id,
                    status: OperationStatus::Failed {
                        error: "Capability authorization denial: operation not granted in CapabilityGrant".into(),
                    },
                    reclaimed_bytes: ByteCount::ZERO,
                    executed_at: UnixTimestamp::now(),
                };
                failed_count += 1;
                results.push(res);
                continue;
            }

            // Step 4: Rate limiter throttling
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

                    // Step 5: Target lock acquisition (mutual exclusion)
                    let _target_lock = match resource_mgr.acquire_target_lock_for_attempt(target_id, attempt_id) {
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

                    // Step 6: Mutation boundary final safety recheck
                    if let Err(e) = safety_gate.validate_mutation_boundary(target, rel_path.as_path(), expected_identity, op.op_id, catalog.generation, authorized.grant.config_generation) {
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

                    // Step 7: Durably commit intent BEFORE mutation (per-op, after all gates)
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
                    if let Err(e) = store.commit_operation_intent(&intent) {
                        let res = OperationFinalResult {
                            op_id: op.op_id,
                            status: OperationStatus::Failed {
                                error: format!("Intent commit failed (mutation not attempted): {}", e),
                            },
                            reclaimed_bytes: ByteCount::ZERO,
                            executed_at: UnixTimestamp::now(),
                        };
                        failed_count += 1;
                        results.push(res);
                        continue;
                    }
                    store.update_intent_state(attempt_id, op.op_id, "MUTATING")?;

                    // Step 8: Physical Destructive Mutation via cached active directory handle
                    let (parent_handle, leaf_name) = match Self::get_or_open_parent_dir(
                        &mut active_dir_cache,
                        target,
                        rel_path.as_path(),
                        resource_mgr,
                    ) {
                        Ok(res) => res,
                        Err(e) => {
                            let res = OperationFinalResult {
                                op_id: op.op_id,
                                status: OperationStatus::Failed {
                                    error: format!("Failed to resolve parent directory: {}", e),
                                },
                                reclaimed_bytes: ByteCount::ZERO,
                                executed_at: UnixTimestamp::now(),
                            };
                            failed_count += 1;
                            results.push(res);
                            continue;
                        }
                    };

                    let mutation_res = parent_handle.unlink_child_file(&leaf_name, expected_identity);

                    // Step 9: Postcondition Verification & Receipt via cached active parent handle
                    let outcome = verifier.verify_with_parent_handle(
                        parent_handle,
                        &leaf_name,
                        expected_identity,
                    );

                    let (status, reclaimed) = match (mutation_res, outcome) {
                        (Ok(_), VerificationOutcome::ConfirmedDeleted | VerificationOutcome::AlreadyGone) => {
                            total_reclaimed = total_reclaimed.saturating_add(*estimated_size);
                            success_count += 1;
                            successful_op_ids.insert(op.op_id);
                            store.update_intent_state(attempt_id, op.op_id, "VERIFIED_SUCCESS")?;
                            (OperationStatus::Success, *estimated_size)
                        }
                        (Err(_), VerificationOutcome::ConfirmedDeleted | VerificationOutcome::AlreadyGone) => {
                            // Object already gone -> Idempotent success, no bytes reclaimed (already absent)
                            success_count += 1;
                            successful_op_ids.insert(op.op_id);
                            store.update_intent_state(attempt_id, op.op_id, "VERIFIED_SUCCESS")?;
                            (OperationStatus::Success, ByteCount::ZERO)
                        }
                        (Ok(_), VerificationOutcome::StillPresent | VerificationOutcome::IdentityMismatch) => {
                            failed_count += 1;
                            store.update_intent_state(attempt_id, op.op_id, "VERIFIED_FAILED")?;
                            (
                                OperationStatus::VerificationFailed {
                                    reason: format!("Postcondition verification failed: {:?}", outcome),
                                },
                                ByteCount::ZERO,
                            )
                        }
                        (Ok(_), outcome) => {
                            // Conservative safety principle (75.md): unconfirmed disk state is RESOLVED_UNKNOWN
                            failed_count += 1;
                            store.update_intent_state(attempt_id, op.op_id, "RESOLVED_UNKNOWN")?;
                            (
                                OperationStatus::VerificationFailed {
                                    reason: format!("Postcondition verification unresolved: {:?}", outcome),
                                },
                                ByteCount::ZERO,
                            )
                        }
                        (Err(e), VerificationOutcome::Unknown | VerificationOutcome::ParentUnavailable | VerificationOutcome::TargetUnavailable) => {
                            failed_count += 1;
                            store.update_intent_state(attempt_id, op.op_id, "RESOLVED_UNKNOWN")?;
                            (
                                OperationStatus::Failed {
                                    error: format!("Mutation error with unknown disk outcome: {}", e),
                                },
                                ByteCount::ZERO,
                            )
                        }
                        (Err(e), _) => {
                            failed_count += 1;
                            store.update_intent_state(attempt_id, op.op_id, "VERIFIED_FAILED")?;
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

                    // Step 10: Record final result in SQLite
                    if let Err(e) = store.record_operation_result(
                        authorized.plan.job_id,
                        authorized.plan.plan_id,
                        target_id,
                        "DELETE_FILE",
                        rel_path.as_str(),
                        expected_identity,
                        *estimated_size,
                        &res,
                    ) {
                        log::error!("CRITICAL: Failed to record operation result in SQLite: {}", e);
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

                    let _target_lock = match resource_mgr.acquire_target_lock_for_attempt(target_id, attempt_id) {
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

                    if let Err(e) = safety_gate.validate_mutation_boundary(target, rel_path.as_path(), expected_identity, op.op_id, catalog.generation, authorized.grant.config_generation) {
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

                    // Step 7: Durably commit intent BEFORE mutation (per-op)
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
                    if let Err(e) = store.commit_operation_intent(&intent) {
                        let res = OperationFinalResult {
                            op_id: op.op_id,
                            status: OperationStatus::Failed {
                                error: format!("Intent commit failed (mutation not attempted): {}", e),
                            },
                            reclaimed_bytes: ByteCount::ZERO,
                            executed_at: UnixTimestamp::now(),
                        };
                        failed_count += 1;
                        results.push(res);
                        continue;
                    }
                    store.update_intent_state(attempt_id, op.op_id, "MUTATING")?;

                    // Step 8: Physical Destructive Mutation via cached active directory handle
                    let (parent_handle, leaf_name) = match Self::get_or_open_parent_dir(
                        &mut active_dir_cache,
                        target,
                        rel_path.as_path(),
                        resource_mgr,
                    ) {
                        Ok(res) => res,
                        Err(e) => {
                            let res = OperationFinalResult {
                                op_id: op.op_id,
                                status: OperationStatus::Failed {
                                    error: format!("Failed to resolve parent directory for rmdir: {}", e),
                                },
                                reclaimed_bytes: ByteCount::ZERO,
                                executed_at: UnixTimestamp::now(),
                            };
                            failed_count += 1;
                            results.push(res);
                            continue;
                        }
                    };

                    let mutation_res = parent_handle.rmdir_child_dir(&leaf_name, expected_identity);

                    // Step 9: Postcondition Verification via active parent handle
                    let outcome = verifier.verify_with_parent_handle(
                        parent_handle,
                        &leaf_name,
                        expected_identity,
                    );

                    // Invalidate active dir cache since directory tree was mutated
                    active_dir_cache = None;

                    let status = match (mutation_res, outcome) {
                        (Ok(_), VerificationOutcome::ConfirmedDeleted | VerificationOutcome::AlreadyGone) => {
                            success_count += 1;
                            successful_op_ids.insert(op.op_id);
                            store.update_intent_state(attempt_id, op.op_id, "VERIFIED_SUCCESS")?;
                            OperationStatus::Success
                        }
                        (Err(_), VerificationOutcome::ConfirmedDeleted | VerificationOutcome::AlreadyGone) => {
                            success_count += 1;
                            successful_op_ids.insert(op.op_id);
                            store.update_intent_state(attempt_id, op.op_id, "VERIFIED_SUCCESS")?;
                            OperationStatus::Success
                        }
                        (Ok(_), VerificationOutcome::StillPresent | VerificationOutcome::IdentityMismatch) => {
                            failed_count += 1;
                            store.update_intent_state(attempt_id, op.op_id, "VERIFIED_FAILED")?;
                            OperationStatus::VerificationFailed {
                                reason: "Rmdir postcondition verification failed: StillPresent/Mismatch".to_string(),
                            }
                        }
                        (Ok(_), outcome) => {
                            // Conservative safety principle (75.md): unconfirmed disk state is RESOLVED_UNKNOWN
                            failed_count += 1;
                            store.update_intent_state(attempt_id, op.op_id, "RESOLVED_UNKNOWN")?;
                            OperationStatus::VerificationFailed {
                                reason: format!("Rmdir postcondition unresolved: {:?}", outcome),
                            }
                        }
                        (Err(e), VerificationOutcome::Unknown | VerificationOutcome::ParentUnavailable | VerificationOutcome::TargetUnavailable) => {
                            failed_count += 1;
                            store.update_intent_state(attempt_id, op.op_id, "RESOLVED_UNKNOWN")?;
                            OperationStatus::Failed {
                                error: format!("Rmdir error with unknown disk outcome: {}", e),
                            }
                        }
                        (Err(e), _) => {
                            failed_count += 1;
                            store.update_intent_state(attempt_id, op.op_id, "VERIFIED_FAILED")?;
                            OperationStatus::Failed {
                                error: format!("Rmdir failed: {}", e),
                            }
                        }
                    };

                    let res = OperationFinalResult {
                        op_id: op.op_id,
                        status,
                        reclaimed_bytes: ByteCount::ZERO,
                        executed_at: UnixTimestamp::now(),
                    };

                    if let Err(e) = store.record_operation_result(
                        authorized.plan.job_id,
                        authorized.plan.plan_id,
                        target_id,
                        "DELETE_DIR_EMPTY",
                        rel_path.as_str(),
                        expected_identity,
                        ByteCount::ZERO,
                        &res,
                    ) {
                        log::error!("CRITICAL: Failed to record dir operation result in SQLite: {}", e);
                    }

                    res
                }
                OperationType::PruneDirRecursive { .. } => {
                    let res = OperationFinalResult {
                        op_id: op.op_id,
                        status: OperationStatus::Skipped {
                            reason: "PruneDirRecursive is deprecated in favor of hierarchical DAG atomic deletions".into(),
                        },
                        reclaimed_bytes: ByteCount::ZERO,
                        executed_at: UnixTimestamp::now(),
                    };
                    skipped_count += 1;
                    res
                }
                OperationType::TrimFilesystem { .. } | OperationType::VacuumDatabase { .. } => {
                    let res = OperationFinalResult {
                        op_id: op.op_id,
                        status: OperationStatus::Skipped {
                            reason: "Maintenance operation delegated to subsystem".into(),
                        },
                        reclaimed_bytes: ByteCount::ZERO,
                        executed_at: UnixTimestamp::now(),
                    };
                    skipped_count += 1;
                    res
                }
            };

            results.push(op_res);
        }

        let elapsed = start_time.elapsed().as_millis() as u64;

        Ok(JobResult {
            job_id: authorized.plan.job_id,
            attempt_id,
            total_reclaimed,
            total_operations: authorized.plan.operations.len(),
            successful_operations: success_count,
            failed_operations: failed_count,
            skipped_operations: skipped_count,
            duration_ms: elapsed,
        })
    }



    /// Resolves and caches the parent directory handle of a relative path under a target root.
    /// Reuses the existing open handle if consecutive operations target the same parent directory.
    fn get_or_open_parent_dir<'a>(
        cache: &'a mut Option<ActiveDirCache>,
        target: &TargetDescriptor,
        rel_path: &Path,
        resource_mgr: &ResourceManager,
    ) -> Result<(&'a SafeDirHandle, String)> {
        let parent_path = rel_path.parent().unwrap_or_else(|| Path::new("")).to_path_buf();
        let leaf_name = rel_path
            .file_name()
            .and_then(|f| f.to_str())
            .ok_or_else(|| CleanerError::SafetyViolation("Invalid UTF-8 in leaf filename".into()))?
            .to_string();

        let matches = match cache {
            Some(c) => c.target_id == target.target_id && c.parent_path == parent_path,
            None => false,
        };

        if !matches {
            let root_permit = resource_mgr.acquire_fd_permit().ok();
            let mut current_dir = SafeDirHandle::open_root_with_permit(&target.base_path, root_permit)?;
            for comp in parent_path.components() {
                let name = comp.as_os_str().to_str().ok_or_else(|| {
                    CleanerError::SafetyViolation("Invalid UTF-8 in relative path component".into())
                })?;
                let child_permit = resource_mgr.acquire_fd_permit().ok();
                current_dir = current_dir.open_child_dir_with_permit(name, child_permit)?;
            }
            *cache = Some(ActiveDirCache {
                target_id: target.target_id.clone(),
                parent_path,
                handle: current_dir,
            });
        }

        Ok((&cache.as_ref().unwrap().handle, leaf_name))
    }
}
