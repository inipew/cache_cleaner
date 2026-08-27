#[cfg(test)]
mod tests {
    use cache_cleaner_daemon::engine::cancellation::{CancelReason, CancellationToken};
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_sequential_first_reason_wins() {
        let token = CancellationToken::new();
        assert!(!token.is_cancelled());
        assert_eq!(token.get_cancel_reason(), None);

        // First cancel
        token.cancel_with_reason(CancelReason::ScreenOn);
        assert!(token.is_cancelled());
        assert_eq!(token.get_cancel_reason(), Some(CancelReason::ScreenOn));

        // Subsequent cancel attempts MUST NOT overwrite the first reason
        token.cancel_with_reason(CancelReason::ThermalCritical);
        assert_eq!(token.get_cancel_reason(), Some(CancelReason::ScreenOn));

        token.cancel_with_reason(CancelReason::ManualCancel);
        assert_eq!(token.get_cancel_reason(), Some(CancelReason::ScreenOn));

        // Reset clears cancellation and reason
        token.reset();
        assert!(!token.is_cancelled());
        assert_eq!(token.get_cancel_reason(), None);
    }

    #[test]
    fn test_concurrent_atomic_cancellation_race() {
        let token = Arc::new(CancellationToken::new());
        let mut handles = Vec::new();

        let reasons = [
            CancelReason::ScreenOn,
            CancelReason::ThermalCritical,
            CancelReason::BatteryLow,
            CancelReason::Unplugged,
            CancelReason::Shutdown,
            CancelReason::Timeout,
            CancelReason::ManualCancel,
            CancelReason::UserActivity,
        ];

        for reason in reasons {
            let t_clone = token.clone();
            handles.push(thread::spawn(move || {
                t_clone.cancel_with_reason(reason);
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        // Exactly one valid reason must be stored and must be one of the attempted reasons
        assert!(token.is_cancelled());
        let stored_reason = token.get_cancel_reason();
        assert!(stored_reason.is_some());
        assert!(reasons.contains(&stored_reason.unwrap()));
    }
}
