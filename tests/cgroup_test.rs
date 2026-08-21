#[cfg(test)]
mod tests {
    use cache_cleaner_daemon::system::cgroup::{
        detect_cgroup_version, get_cgroup_diagnostics, migrate_to_background_cgroup, CgroupVersion,
    };

    #[test]
    fn test_cgroup_version_detection_does_not_panic() {
        let version = detect_cgroup_version();
        // Should return a valid variant without panicking on Linux / host environments
        assert!(matches!(
            version,
            CgroupVersion::V1 | CgroupVersion::V2 | CgroupVersion::Hybrid | CgroupVersion::None
        ));
    }

    #[test]
    fn test_cgroup_diagnostics_structure() {
        let diag = get_cgroup_diagnostics();
        assert!(!diag.detected_version.is_empty());
        // active_reclaim_paths should match supports_memory_reclaim boolean logic
        if diag.supports_memory_reclaim {
            assert!(!diag.active_reclaim_paths.is_empty());
        } else {
            assert!(diag.active_reclaim_paths.is_empty());
        }
    }

    #[test]
    fn test_cgroup_migration_execution() {
        let summary = migrate_to_background_cgroup();
        // Running on host / CI might not have Android background cgroups, but must return cleanly
        assert!(summary.version.is_some());
    }
}
