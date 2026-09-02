use serde::{Deserialize, Serialize};
use std::fmt;

use crate::domain::candidate::SafetyValidatedCandidate;
use crate::domain::types::UnixTimestamp;

/// Reason codes explaining why a policy decision was made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecisionReason {
    /// Matches cache classification and exceeds minimum age threshold
    ExceedsRetentionAge,
    /// Storage space emergency pressure requires aggressive cleanup
    StoragePressureThreshold,
    /// Item is protected because its age is within the grace period
    WithinRetentionGracePeriod,
    /// Application or package is explicitly whitelisted
    PackageWhitelisted,
    /// File type is protected bytecode/JIT metadata
    ProtectedBytecode,
    /// Target is classified as read-only inspection
    TargetIsReadOnly,
    /// Exceeds job execution time or item budget
    BudgetExceeded,
    /// Custom configuration rule matched
    RuleMatch(String),
}

impl fmt::Display for DecisionReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExceedsRetentionAge => write!(f, "Exceeds retention age"),
            Self::StoragePressureThreshold => write!(f, "Storage pressure threshold triggered"),
            Self::WithinRetentionGracePeriod => write!(f, "Within retention grace period"),
            Self::PackageWhitelisted => write!(f, "Package is whitelisted"),
            Self::ProtectedBytecode => write!(f, "Protected bytecode/JIT artifact"),
            Self::TargetIsReadOnly => write!(f, "Target is read-only"),
            Self::BudgetExceeded => write!(f, "Execution budget exceeded"),
            Self::RuleMatch(rule) => write!(f, "Rule match: {}", rule),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyPermit {
    pub candidate: SafetyValidatedCandidate,
    pub priority: u32,
    pub reason: DecisionReason,
    pub decided_at: UnixTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyDeny {
    pub candidate: SafetyValidatedCandidate,
    pub reason: DecisionReason,
    pub decided_at: UnixTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicySkip {
    pub candidate: SafetyValidatedCandidate,
    pub reason: DecisionReason,
    pub decided_at: UnixTimestamp,
}

/// Strongly-typed deterministic Policy Decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PolicyDecision {
    Allow(PolicyPermit),
    Deny(PolicyDeny),
    Skip(PolicySkip),
}

impl PolicyDecision {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allow(_))
    }

    pub fn is_denied(&self) -> bool {
        matches!(self, Self::Deny(_))
    }
}
