#[cfg(test)]
mod tests {
    use cache_cleaner_daemon::system::freezer::{
        detect_freezer_version, enumerate_frozen_uids, get_freezer_diagnostics,
        get_pid_freezer_state, get_uid_freezer_state, is_cached_apps_freezer_enabled,
        is_freezer_supported, FreezerState,
    };

    #[test]
    fn test_freezer_detection_does_not_panic() {
        let version = detect_freezer_version();
        assert!(version == "v2" || version == "v1" || version == "none");

        let supported = is_freezer_supported();
        if version == "none" {
            assert!(!supported);
        } else {
            assert!(supported);
        }

    }

    #[test]
    fn test_cached_apps_freezer_enabled_check() {
        // Must return boolean without panic
        let _ = is_cached_apps_freezer_enabled();
    }

    #[test]
    fn test_enumerate_frozen_uids() {
        let frozen_set = enumerate_frozen_uids();
        // Returned hashset contains valid uids
        for uid in &frozen_set {
            assert!(*uid > 0);
        }
    }

    #[test]
    fn test_uid_freezer_state_query() {
        let state = get_uid_freezer_state(10001);
        assert!(matches!(
            state,
            FreezerState::Frozen
                | FreezerState::Thawed
                | FreezerState::Freezing
                | FreezerState::Unsupported
        ));
    }

    #[test]
    fn test_pid_freezer_state_query() {
        let state = get_pid_freezer_state(std::process::id());
        assert!(matches!(
            state,
            FreezerState::Frozen
                | FreezerState::Thawed
                | FreezerState::Freezing
                | FreezerState::Unsupported
        ));
    }

    #[test]
    fn test_freezer_diagnostics_structure() {
        let diag = get_freezer_diagnostics();
        assert!(!diag.freezer_cgroup_version.is_empty());
        assert_eq!(diag.total_frozen_uids_count, diag.frozen_uids.len());
    }
}
