#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use cache_cleaner_daemon::system::psi::{
        evaluate_current_pressure_level, get_psi_diagnostics, is_psi_supported, parse_psi_content,
        read_memory_pressure, PsiWatcher,
    };

    #[test]
    fn test_parse_psi_content_sample() {
        let sample = "some avg10=2.34 avg60=1.50 avg300=0.85 total=1234567\nfull avg10=0.15 avg60=0.05 avg300=0.00 total=45678\n";
        let metrics = parse_psi_content(sample).expect("Failed to parse valid PSI content");

        assert_eq!(metrics.some.avg10, 2.34);
        assert_eq!(metrics.some.avg60, 1.50);
        assert_eq!(metrics.some.avg300, 0.85);
        assert_eq!(metrics.some.total_us, 1_234_567);

        let full = metrics.full.expect("Full metric sample missing");
        assert_eq!(full.avg10, 0.15);
        assert_eq!(full.avg60, 0.05);
        assert_eq!(full.avg300, 0.00);
        assert_eq!(full.total_us, 45678);
    }

    #[test]
    fn test_parse_psi_some_only_sample() {
        // CPU pressure files only have "some", no "full"
        let sample = "some avg10=12.50 avg60=8.20 avg300=4.10 total=999999\n";
        let metrics = parse_psi_content(sample).expect("Failed to parse some-only PSI content");

        assert_eq!(metrics.some.avg10, 12.50);
        assert!(metrics.full.is_none());
    }


    #[test]
    fn test_parse_invalid_psi_content() {
        let invalid = "this is not valid psi data";
        assert!(parse_psi_content(invalid).is_none());
    }

    #[test]
    fn test_psi_diagnostics_query() {
        let diag = get_psi_diagnostics();
        assert!(!diag.current_level.is_empty());
        if is_psi_supported() {
            assert!(diag.is_supported);
            let _ = read_memory_pressure();
            let _ = evaluate_current_pressure_level();
        }
    }

    #[test]
    fn test_psi_watcher_lifecycle_and_cooldown() {
        let mut watcher = PsiWatcher::create(150, 250);
        // Initially cooldown must allow immediate response
        assert!(watcher.can_respond(45));

        watcher.record_response();
        // Immediately after responding, cooldown should prevent another response
        assert!(!watcher.can_respond(45));
    }
}
