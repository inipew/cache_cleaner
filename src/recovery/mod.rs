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
        let mut recovered_count = 0;

        for (attempt_id, job_id) in uncompleted {
            log::info!("Reconciling interrupted attempt {} for job {}", attempt_id, job_id);

            let intents = store.get_operation_intents_for_attempt(attempt_id)?;
            let mut total_reclaimed = ByteCount::ZERO;

            for intent in intents {
                let target = match snapshot.get(&intent.target_id) {
                    Some(t) => t,
                    None => {
                        log::warn!("Target {} no longer available during recovery", intent.target_id);
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
                        (OperationStatus::Success, intent.estimated_bytes)
                    }
                    VerificationOutcome::StillPresent | VerificationOutcome::IdentityMismatch => (
                        OperationStatus::Failed {
                            error: format!("Unfinished mutation post-crash: {:?}", outcome),
                        },
                        ByteCount::ZERO,
                    ),
                    VerificationOutcome::ParentUnavailable
                    | VerificationOutcome::TargetUnavailable
                    | VerificationOutcome::Unknown => (
                        OperationStatus::Skipped {
                            reason: format!("Outcome: {:?}", outcome),
                        },
                        ByteCount::ZERO,
                    ),
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

            store.update_attempt_state(attempt_id, "RECONCILED")?;
            store.update_job_state(job_id, "RECOVERED", total_reclaimed)?;
            recovered_count += 1;
        }

        Ok(recovered_count)
    }
}
