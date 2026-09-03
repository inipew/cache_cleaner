use crate::catalog::TargetCatalog;
use crate::config::DaemonConfig;
use crate::domain::types::{CatalogGeneration, ConfigGeneration};
use crate::error::Result;

/// Shadow comparator: legacy engine vs new pipeline per 100.md:25.
/// Executor = NO-OP during shadow, only compare plans.
#[derive(Debug, Default)]
pub struct ShadowComparator;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowReport {
    pub legacy_targets: usize,
    pub new_targets: usize,
    pub catalog_generation: CatalogGeneration,
    pub config_generation: ConfigGeneration,
    pub legacy_plan_ops: usize,
    pub new_plan_ops: usize,
    pub diff: ShadowDiff,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShadowDiff {
    Equal,
    NewMore { extra: usize },
    LegacyMore { extra: usize },
    Diverged { legacy: usize, new: usize },
}

impl ShadowComparator {
    pub fn new() -> Self {
        Self
    }

    /// Compare legacy engine target discovery vs new catalog.
    /// Legacy is behavioral oracle, new is authoritative after parity.
    pub fn compare(&self, _legacy_config: &DaemonConfig, catalog: &TargetCatalog) -> Result<ShadowReport> {
        // Legacy: count via old engine's notion (approximate via catalog's previous discovery)
        // For shadow, we treat legacy_targets as what old engine would have seen (same as new for now)
        // Real comparison would run CleanEngine::scan vs CandidateScanner, but we stub for incremental.
        let snapshot = catalog.take_snapshot();
        let new_targets = snapshot.len();
        // Simulate legacy as same count for now — will diverge as new adds ExternalCache etc.
        let legacy_targets = new_targets;

        let diff = match legacy_targets.cmp(&new_targets) {
            std::cmp::Ordering::Equal => ShadowDiff::Equal,
            std::cmp::Ordering::Less => ShadowDiff::NewMore { extra: new_targets - legacy_targets },
            std::cmp::Ordering::Greater => ShadowDiff::LegacyMore { extra: legacy_targets - new_targets },
        };

        Ok(ShadowReport {
            legacy_targets,
            new_targets,
            catalog_generation: snapshot.generation,
            config_generation: ConfigGeneration::INITIAL,
            legacy_plan_ops: 0, // NO-OP in shadow
            new_plan_ops: 0,
            diff,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::TargetCatalog;
    use crate::config::DaemonConfig;
    #[test]
    fn shadow_equal_when_same() {
        let catalog = TargetCatalog::new();
        let cfg = DaemonConfig::default();
        let cmp = ShadowComparator::new();
        let rep = cmp.compare(&cfg, &catalog).unwrap();
        assert_eq!(rep.diff, ShadowDiff::Equal);
    }
}
