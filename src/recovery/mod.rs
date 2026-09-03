use crate::catalog::TargetCatalog;
use crate::domain::result::{OperationFinalResult, OperationStatus};
use crate::domain::types::{ByteCount, UnixTimestamp};
use crate::error::Result;
use crate::store::SqliteStore;
use crate::verifier::{PostconditionVerifier, VerificationOutcome};

/// Startup Crash Recovery Engine.
/// Reconciles interrupted jobs by inspecting durable OperationIntents in SQLite and verifying physical storage truth.
#[derive(Debug, Default)]
pub struct RecoveryEngine;

impl RecoveryEngine {
    pub fn new() -> Self {
        Self
    }

    /// Scans SQLite store for interrupted attempts and reconciles them idempotently against disk state.
    /// Only marks attempt as RECONCILED if 100% of intents are verified deleted on storage.
    pub fn reconcile_startup_crashes(
        &self,
        store: &SqliteStore,
        catalog: &TargetCatalog,
        verifier: &PostconditionVerifier,
    ) -> Result<usize> {
        let uncompleted = store.get_uncompleted_attempts()?;
        if uncompleted.is_empty() {
            return Ok(0);
        }

        let snapshot = catalog.take_snapshot();
        let mut fully_recovered_count = 0;

        for (attempt_id, job_id) in uncompleted {
            log::info!("Reconciling interrupted attempt {} for job {}", attempt_id, job_id);

            let intents = store.get_operation_intents_for_attempt(attempt_id)?;
            let mut total_reclaimed = ByteCount::ZERO;
            let mut all_succeeded = true;
            let mut has_unknown = false;

            for intent in intents {
                let target = match snapshot.get(&intent.target_id) {
                    Some(t) => t,
                    None => {
                        log::warn!("Target {} no longer available during recovery", intent.target_id);
                        all_succeeded = false;
                        has_unknown = true;
                        let _ = store.update_intent_state(attempt_id, intent.op_id, "RESOLVED_UNKNOWN");
                        continue;
                    }
                };

                let outcome = verifier.verify_operation_postcondition(
                    &target.base_path,
                    intent.rel_path.as_path(),
                    &intent.expected_identity,
                );

                let (status, reclaimed) = match outcome {
                    VerificationOutcome::ConfirmedDeleted | VerificationOutcome::AlreadyGone => {
                        total_reclaimed = total_reclaimed.saturating_add(intent.estimated_bytes);
                        let _ = store.update_intent_state(attempt_id, intent.op_id, "VERIFIED_SUCCESS");
                        (OperationStatus::Success, intent.estimated_bytes)
                    }
                    VerificationOutcome::StillPresent | VerificationOutcome::IdentityMismatch => {
                        all_succeeded = false;
                        let _ = store.update_intent_state(attempt_id, intent.op_id, "VERIFIED_FAILED");
                        (
                            OperationStatus::Failed {
                                error: format!("Unfinished mutation post-crash: {:?}", outcome),
                            },
                            ByteCount::ZERO,
                        )
                    }
                    VerificationOutcome::ParentUnavailable
                    | VerificationOutcome::TargetUnavailable
                    | VerificationOutcome::Stale
                    | VerificationOutcome::Unknown => {
                        all_succeeded = false;
                        has_unknown = true;
                        let _ = store.update_intent_state(attempt_id, intent.op_id, "RESOLVED_UNKNOWN");
                        (
                            OperationStatus::Skipped {
                                reason: format!("Post-crash verification outcome: {:?}", outcome),
                            },
                            ByteCount::ZERO,
                        )
                    }
                };

                let op_res = OperationFinalResult {
                    op_id: intent.op_id,
                    status,
                    reclaimed_bytes: reclaimed,
                    executed_at: UnixTimestamp::now(),
                };

                let _ = store.record_operation_result(
                    job_id,
                    crate::domain::types::PlanId(1),
                    &intent.target_id,
                    &intent.mutation_type.to_string(),
                    intent.rel_path.as_str(),
                    &intent.expected_identity,
                    intent.estimated_bytes,
                    &op_res,
                );
            }

            if all_succeeded {
                store.update_attempt_state(attempt_id, "RECONCILED")?;
                store.update_job_state(job_id, "RECOVERED", total_reclaimed)?;
                fully_recovered_count += 1;
            } else if has_unknown {
                store.update_attempt_state(attempt_id, "UNRESOLVED")?;
                store.update_job_state(job_id, "PARTIALLY_RECONCILED", total_reclaimed)?;
            } else {
                store.update_attempt_state(attempt_id, "FAILED")?;
                store.update_job_state(job_id, "FAILED", total_reclaimed)?;
            }
        }

        Ok(fully_recovered_count)
    }
}
