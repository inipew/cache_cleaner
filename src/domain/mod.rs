pub mod candidate;
pub mod decision;
pub mod grant;
pub mod intent;
pub mod job;
pub mod plan;
pub mod result;
pub mod target;
pub mod types;

// Flattened re-exports for convenience
pub use candidate::{Candidate, SafetyValidatedCandidate};
pub use decision::{DecisionReason, PolicyDecision, PolicyDeny, PolicyPermit, PolicySkip};
pub use grant::{AuthorizedPlan, Capability, CapabilityGrant};
pub use intent::{MutationType, OperationIntent};
pub use job::{JobAttempt, JobState};
pub use plan::{OperationType, PlannedOperation, PlannedPlan};
pub use result::{JobResult, OperationFinalResult, OperationStatus};
pub use target::{TargetClass, TargetDescriptor, TargetSafetyTier};
pub use types::{
    AttemptId, ByteCount, CandidateId, DeviceNumber, FileIdentity, GenerationId, GrantId,
    InodeNumber, JobId, OperationId, PlanId, RelativePath, TargetId, UnixTimestamp, WorkerId,
};
