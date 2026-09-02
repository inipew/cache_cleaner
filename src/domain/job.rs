use serde::{Deserialize, Serialize};
use std::fmt;

use crate::domain::types::{AttemptId, JobId, UnixTimestamp};

/// Formal lifecycle state machine of a cleanup Job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobState {
    /// Job created and registered in queue, awaiting scheduler admission.
    Pending,
    /// Admitted by scheduler with resource reservations.
    Admitted,
    /// Scanner is actively discovering candidate objects.
    Scanning,
    /// Policy evaluated and Planner is generating operation graph.
    Planning,
    /// Capability grants issued and plan is authorized.
    Authorized,
    /// Single mutation boundary executor is executing operations.
    Executing,
    /// Verifier is checking postconditions on physical storage.
    Verifying,
    /// Terminal state: All planned operations executed, verified, and recorded.
    Completed,
    /// Terminal state: Job failed with an unrecoverable error.
    Failed { reason: String },
    /// Terminal state: Job aborted/cancelled due to preemption or user request.
    Aborted { reason: String },
}

impl JobState {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed { .. } | Self::Aborted { .. })
    }

    pub fn can_transition_to(&self, next: &JobState) -> bool {
        match (self, next) {
            // Pending can move to Admitted, Failed, or Aborted
            (JobState::Pending, JobState::Admitted) => true,
            (JobState::Pending, JobState::Failed { .. }) => true,
            (JobState::Pending, JobState::Aborted { .. }) => true,

            // Admitted can move to Scanning, Failed, or Aborted
            (JobState::Admitted, JobState::Scanning) => true,
            (JobState::Admitted, JobState::Failed { .. }) => true,
            (JobState::Admitted, JobState::Aborted { .. }) => true,

            // Scanning can move to Planning, Failed, or Aborted
            (JobState::Scanning, JobState::Planning) => true,
            (JobState::Scanning, JobState::Failed { .. }) => true,
            (JobState::Scanning, JobState::Aborted { .. }) => true,

            // Planning can move to Authorized, Completed (if 0 items), Failed, or Aborted
            (JobState::Planning, JobState::Authorized) => true,
            (JobState::Planning, JobState::Completed) => true, // Empty plan
            (JobState::Planning, JobState::Failed { .. }) => true,
            (JobState::Planning, JobState::Aborted { .. }) => true,

            // Authorized can move to Executing, Failed, or Aborted
            (JobState::Authorized, JobState::Executing) => true,
            (JobState::Authorized, JobState::Failed { .. }) => true,
            (JobState::Authorized, JobState::Aborted { .. }) => true,

            // Executing can move to Verifying, Failed, or Aborted
            (JobState::Executing, JobState::Verifying) => true,
            (JobState::Executing, JobState::Failed { .. }) => true,
            (JobState::Executing, JobState::Aborted { .. }) => true,

            // Verifying can move to Completed, Failed, or Aborted
            (JobState::Verifying, JobState::Completed) => true,
            (JobState::Verifying, JobState::Failed { .. }) => true,
            (JobState::Verifying, JobState::Aborted { .. }) => true,

            // Terminal states cannot transition to anything
            _ => false,
        }
    }
}

impl fmt::Display for JobState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => write!(f, "Pending"),
            Self::Admitted => write!(f, "Admitted"),
            Self::Scanning => write!(f, "Scanning"),
            Self::Planning => write!(f, "Planning"),
            Self::Authorized => write!(f, "Authorized"),
            Self::Executing => write!(f, "Executing"),
            Self::Verifying => write!(f, "Verifying"),
            Self::Completed => write!(f, "Completed"),
            Self::Failed { reason } => write!(f, "Failed({})", reason),
            Self::Aborted { reason } => write!(f, "Aborted({})", reason),
        }
    }
}

/// Execution attempt metadata for audit and idempotency tracking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobAttempt {
    pub attempt_id: AttemptId,
    pub job_id: JobId,
    pub attempt_number: u32,
    pub started_at: UnixTimestamp,
    pub finished_at: Option<UnixTimestamp>,
    pub state: JobState,
}

impl JobAttempt {
    pub fn new(attempt_id: AttemptId, job_id: JobId, attempt_number: u32) -> Self {
        Self {
            attempt_id,
            job_id,
            attempt_number,
            started_at: UnixTimestamp::now(),
            finished_at: None,
            state: JobState::Pending,
        }
    }

    pub fn transition_to(&mut self, next: JobState) -> Result<(), String> {
        if self.state.can_transition_to(&next) {
            if next.is_terminal() {
                self.finished_at = Some(UnixTimestamp::now());
            }
            self.state = next;
            Ok(())
        } else {
            Err(format!(
                "Illegal state transition from '{}' to '{}' for attempt {}",
                self.state, next, self.attempt_id
            ))
        }
    }
}
