use crate::config_pipeline::EffectiveConfig;
use crate::domain::candidate::SafetyValidatedCandidate;
use crate::domain::decision::{
    DecisionReason, PolicyDecision, PolicyDeny, PolicyPermit,
};
use crate::domain::target::TargetDescriptor;
use crate::domain::types::UnixTimestamp;

/// Policy Engine evaluating configurable business rules, age thresholds, and whitelists.
#[derive(Debug, Default)]
pub struct PolicyEngine;

impl PolicyEngine {
    pub fn new() -> Self {
        Self
    }

    /// Evaluates a safety-validated candidate against effective policy rules.
    pub fn evaluate_candidate(
        &self,
        validated: SafetyValidatedCandidate,
        target: &TargetDescriptor,
        config: &EffectiveConfig,
        now: UnixTimestamp,
    ) -> PolicyDecision {
        self.evaluate_candidate_with_freezer(validated, target, config, now, false)
    }

    /// Evaluates a safety-validated candidate considering frozen app priority.
    pub fn evaluate_candidate_with_freezer(
        &self,
        validated: SafetyValidatedCandidate,
        target: &TargetDescriptor,
        config: &EffectiveConfig,
        now: UnixTimestamp,
        is_frozen: bool,
    ) -> PolicyDecision {
        let decided_at = UnixTimestamp::now();

        // Rule 1: Package Whitelist Check
        if let Some(ref pkg) = target.package_name {
            if config.is_package_whitelisted(pkg) {
                return PolicyDecision::Deny(PolicyDeny {
                    candidate: validated,
                    reason: DecisionReason::PackageWhitelisted,
                    decided_at,
                });
            }
        }

        // Rule 2: Bytecode / JIT Cache Protection Check
        let rel_str = validated.candidate.rel_path.as_str();
        if rel_str.ends_with(".dex")
            || rel_str.ends_with(".oat")
            || rel_str.ends_with(".art")
            || rel_str.ends_with(".vdex")
            || rel_str.contains("code_cache")
        {
            return PolicyDecision::Deny(PolicyDeny {
                candidate: validated,
                reason: DecisionReason::ProtectedBytecode,
                decided_at,
            });
        }

        // Rule 3: Age Retention Threshold Check
        let age_secs = validated.candidate.mtime.age_secs(now);
        if age_secs < config.validated.min_app_cache_age_secs {
            return PolicyDecision::Deny(PolicyDeny {
                candidate: validated,
                reason: DecisionReason::WithinRetentionGracePeriod,
                decided_at,
            });
        }

        // Rule passed: Allow candidate for planning (Frozen apps get priority 200, active get 100)
        let priority = if is_frozen { 200 } else { 100 };

        PolicyDecision::Allow(PolicyPermit {
            candidate: validated,
            priority,
            reason: DecisionReason::ExceedsRetentionAge,
            decided_at,
        })
    }
}
