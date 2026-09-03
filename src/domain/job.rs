use serde::{Deserialize, Serialize};
use std::fmt;

use crate::domain::types::{
    AttemptId, CatalogGeneration, ConfigGeneration, JobId, UnixTimestamp,
};

/// Formal lifecycle state machine of a cleanup Job — expanded per 67.md.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobState {
    Pending,
    Queued,
    Admitted,
    Preparing,
    Scanning,
    Planning,
    AwaitingAuthorization,
    Authorized,
    WaitingResources,
    Executing,
    Verifying,
    Reconciling,
    Completed,
    Failed { reason: String },
    Aborted { reason: String },
    Cancelled { reason: String },
    TimedOut { reason: String },
    RecoveryRequired { reason: String },
    Stale { reason: String },
}

impl JobState {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed
                | Self::Failed { .. }
                | Self::Aborted { .. }
                | Self::Cancelled { .. }
                | Self::TimedOut { .. }
                | Self::RecoveryRequired { .. }
                | Self::Stale { .. }
        )
    }

    pub fn can_transition_to(&self, next: &JobState) -> bool {
        match (self, next) {
            (JobState::Pending, JobState::Queued) => true,
            (JobState::Pending, JobState::Admitted) => true,
            (JobState::Pending, JobState::Failed { .. }) => true,
            (JobState::Pending, JobState::Aborted { .. }) => true,
            (JobState::Pending, JobState::Cancelled { .. }) => true,
            (JobState::Queued, JobState::Admitted) => true,
            (JobState::Queued, JobState::Failed { .. }) => true,
            (JobState::Queued, JobState::Aborted { .. }) => true,
            (JobState::Admitted, JobState::Preparing) => true,
            (JobState::Admitted, JobState::Scanning) => true,
            (JobState::Admitted, JobState::Failed { .. }) => true,
            (JobState::Admitted, JobState::Aborted { .. }) => true,
            (JobState::Preparing, JobState::Scanning) => true,
            (JobState::Preparing, JobState::Failed { .. }) => true,
            (JobState::Scanning, JobState::Planning) => true,
            (JobState::Scanning, JobState::Failed { .. }) => true,
            (JobState::Scanning, JobState::Aborted { .. }) => true,
            (JobState::Planning, JobState::AwaitingAuthorization) => true,
            (JobState::Planning, JobState::Authorized) => true,
            (JobState::Planning, JobState::Completed) => true,
            (JobState::Planning, JobState::Failed { .. }) => true,
            (JobState::Planning, JobState::Aborted { .. }) => true,
            (JobState::AwaitingAuthorization, JobState::Authorized) => true,
            (JobState::AwaitingAuthorization, JobState::Failed { .. }) => true,
            (JobState::Authorized, JobState::WaitingResources) => true,
            (JobState::Authorized, JobState::Executing) => true,
            (JobState::Authorized, JobState::Failed { .. }) => true,
            (JobState::Authorized, JobState::Aborted { .. }) => true,
            (JobState::WaitingResources, JobState::Executing) => true,
            (JobState::WaitingResources, JobState::Failed { .. }) => true,
            (JobState::Executing, JobState::Verifying) => true,
            (JobState::Executing, JobState::Reconciling) => true,
            (JobState::Executing, JobState::Failed { .. }) => true,
            (JobState::Executing, JobState::Aborted { .. }) => true,
            (JobState::Verifying, JobState::Completed) => true,
            (JobState::Verifying, JobState::Reconciling) => true,
            (JobState::Verifying, JobState::Failed { .. }) => true,
            (JobState::Verifying, JobState::Aborted { .. }) => true,
            (JobState::Verifying, JobState::RecoveryRequired { .. }) => true,
            (JobState::Reconciling, JobState::Completed) => true,
            (JobState::Reconciling, JobState::Failed { .. }) => true,
            (JobState::Reconciling, JobState::RecoveryRequired { .. }) => true,
            // Any non-terminal can go to Cancelled/TimedOut/Stale
            (_, JobState::Cancelled { .. }) if !self.is_terminal() => true,
            (_, JobState::TimedOut { .. }) if !self.is_terminal() => true,
            (_, JobState::Stale { .. }) if !self.is_terminal() => true,
            (_, JobState::RecoveryRequired { .. }) if !self.is_terminal() => true,
            _ => false,
        }
    }
}

impl fmt::Display for JobState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => write!(f, "Pending"),
            Self::Queued => write!(f, "Queued"),
            Self::Admitted => write!(f, "Admitted"),
            Self::Preparing => write!(f, "Preparing"),
            Self::Scanning => write!(f, "Scanning"),
            Self::Planning => write!(f, "Planning"),
            Self::AwaitingAuthorization => write!(f, "AwaitingAuthorization"),
            Self::Authorized => write!(f, "Authorized"),
            Self::WaitingResources => write!(f, "WaitingResources"),
            Self::Executing => write!(f, "Executing"),
            Self::Verifying => write!(f, "Verifying"),
            Self::Reconciling => write!(f, "Reconciling"),
            Self::Completed => write!(f, "Completed"),
            Self::Failed { reason } => write!(f, "Failed({})", reason),
            Self::Aborted { reason } => write!(f, "Aborted({})", reason),
            Self::Cancelled { reason } => write!(f, "Cancelled({})", reason),
            Self::TimedOut { reason } => write!(f, "TimedOut({})", reason),
            Self::RecoveryRequired { reason } => write!(f, "RecoveryRequired({})", reason),
            Self::Stale { reason } => write!(f, "Stale({})", reason),
        }
    }
}

/// Execution attempt metadata for audit and idempotency tracking — with generation binding and version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobAttempt {
    pub attempt_id: AttemptId,
    pub job_id: JobId,
    pub attempt_number: u32,
    pub started_at: UnixTimestamp,
    pub finished_at: Option<UnixTimestamp>,
    pub state: JobState,
    pub state_version: u64,
    pub catalog_generation: CatalogGeneration,
    pub config_generation: ConfigGeneration,
    pub plan_id: Option<crate::domain::types::PlanId>,
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
            state_version: 1,
            catalog_generation: CatalogGeneration::INITIAL,
            config_generation: ConfigGeneration::INITIAL,
            plan_id: None,
        }
    }

    pub fn new_with_generations(
        attempt_id: AttemptId,
        job_id: JobId,
        attempt_number: u32,
        catalog_generation: CatalogGeneration,
        config_generation: ConfigGeneration,
    ) -> Self {
        Self {
            attempt_id,
            job_id,
            attempt_number,
            started_at: UnixTimestamp::now(),
            finished_at: None,
            state: JobState::Pending,
            state_version: 1,
            catalog_generation,
            config_generation,
            plan_id: None,
        }
    }

    pub fn transition_to(&mut self, next: JobState) -> Result<(), String> {
        self.transition_with_cas(next, self.state_version)
    }

    pub fn transition_with_cas(&mut self, next: JobState, expected_version: u64) -> Result<(), String> {
        if self.state_version != expected_version {
            return Err(format!(
                "CAS failed for attempt {}: expected version {}, actual {}",
                self.attempt_id, expected_version, self.state_version
            ));
        }
        if self.state.can_transition_to(&next) {
            if next.is_terminal() {
                self.finished_at = Some(UnixTimestamp::now());
            }
            self.state = next;
            self.state_version += 1;
            Ok(())
        } else {
            Err(format!(
                "Illegal state transition from '{}' to '{}' for attempt {}",
                self.state, next, self.attempt_id
            ))
        }
    }
}
