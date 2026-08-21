#[cfg(test)]
mod tests {
    use cache_cleaner_daemon::config::DaemonConfig;
    use cache_cleaner_daemon::system::{DaemonContext, DaemonState};

    #[test]
    fn test_daemon_state_initialization_and_display() {
        let config = DaemonConfig::default();
        let ctx = DaemonContext::new(config, None);

        assert_eq!(ctx.get_state(), DaemonState::Idle);
        assert_eq!(format!("{}", ctx.get_state()), "Idle / Sleeping");
    }

    #[test]
    fn test_daemon_state_transitions() {
        let config = DaemonConfig::default();
        let ctx = DaemonContext::new(config, None);

        // Transition: Idle -> EvaluatingTriggers
        ctx.set_state(DaemonState::EvaluatingTriggers);
        assert_eq!(ctx.get_state(), DaemonState::EvaluatingTriggers);
        assert_eq!(format!("{}", ctx.get_state()), "Evaluating Triggers");

        // Transition: EvaluatingTriggers -> CleaningScheduled
        ctx.set_state(DaemonState::CleaningScheduled);
        assert_eq!(ctx.get_state(), DaemonState::CleaningScheduled);
        assert_eq!(
            format!("{}", ctx.get_state()),
            "Cleaning in progress (Scheduled)"
        );

        // Transition: CleaningScheduled -> Preempted
        ctx.set_state(DaemonState::Preempted("Screen turned ON".to_string()));
        assert_eq!(
            ctx.get_state(),
            DaemonState::Preempted("Screen turned ON".to_string())
        );
        assert_eq!(
            format!("{}", ctx.get_state()),
            "Preempted (Screen turned ON)"
        );

        // Transition: Preempted -> PressureReclaiming
        ctx.set_state(DaemonState::PressureReclaiming("Critical".to_string()));
        assert_eq!(
            ctx.get_state(),
            DaemonState::PressureReclaiming("Critical".to_string())
        );
        assert_eq!(
            format!("{}", ctx.get_state()),
            "Memory Reclaim in progress (Critical)"
        );

        // Transition -> ShuttingDown
        ctx.trigger_shutdown();
        assert_eq!(ctx.get_state(), DaemonState::ShuttingDown);
        assert!(ctx.is_shutdown_requested());
        assert!(ctx.cancel_token.is_cancelled());
    }

    #[test]
    fn test_state_serialization_deserialization() {
        let state = DaemonState::CleaningManual;
        let serialized = serde_json::to_string(&state).expect("Failed to serialize DaemonState");
        let deserialized: DaemonState =
            serde_json::from_str(&serialized).expect("Failed to deserialize DaemonState");

        assert_eq!(state, deserialized);
    }
}
