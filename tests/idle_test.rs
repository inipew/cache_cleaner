#[cfg(test)]
mod tests {
    use cache_cleaner_daemon::hardware::ScreenState;
    use cache_cleaner_daemon::system::idle::{
        IdleBlocker, IdleContext, IdleManager, IdlePolicy, IdlePositive, IdleState,
        MaintenanceEligibility, SensorReading, ThermalHysteresisState,
    };
    use cache_cleaner_daemon::util::{Clock, FakeClock};
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn test_idle_score_boundedness_and_saturation() {
        // Maximum possible positive conditions
        let ctx = IdleContext {
            screen: ScreenState::Off,
            screen_off_duration: Some(Duration::from_secs(600)),
            charging: true,
            battery_percent: 100,
            cpu_psi_pct: SensorReading::available(0.1),
            io_psi_pct: SensorReading::available(0.1),
            mem_psi_pct: SensorReading::available(0.1),
            thermal_celsius: SensorReading::available(30.0),
            thermal_source: Some("battery".to_string()),
            stationary: true,
            user_active: false,
        };

        let assessment = IdlePolicy::evaluate(
            &ctx,
            IdleState::Active,
            ThermalHysteresisState::Normal,
            Duration::from_secs(300),
        );

        // Score must be capped at 100 exactly
        assert_eq!(assessment.score, 100);
        assert!(assessment.score <= 100);
        assert_eq!(assessment.state, IdleState::DeepIdle);
        assert_eq!(
            assessment.standard_maintenance,
            MaintenanceEligibility::Allowed
        );
        assert_eq!(assessment.heavy_maintenance, MaintenanceEligibility::Allowed);
        assert!(assessment.blockers.is_empty());
    }

    #[test]
    fn test_hard_gate_grace_period_blocks_maintenance_despite_max_score() {
        // Screen is OFF, but only for 30 seconds (< 5 min grace)
        let ctx = IdleContext {
            screen: ScreenState::Off,
            screen_off_duration: Some(Duration::from_secs(30)),
            charging: true,
            battery_percent: 95,
            cpu_psi_pct: SensorReading::available(0.5),
            io_psi_pct: SensorReading::available(0.2),
            mem_psi_pct: SensorReading::available(0.1),
            thermal_celsius: SensorReading::available(32.0),
            thermal_source: Some("soc".to_string()),
            stationary: true,
            user_active: false,
        };

        let assessment = IdlePolicy::evaluate(
            &ctx,
            IdleState::Active,
            ThermalHysteresisState::Normal,
            Duration::from_secs(300),
        );

        // State MUST be IdleCandidate
        assert_eq!(assessment.state, IdleState::IdleCandidate);
        // Maintenance MUST be strictly blocked
        assert_eq!(
            assessment.standard_maintenance,
            MaintenanceEligibility::Blocked
        );
        assert_eq!(
            assessment.heavy_maintenance,
            MaintenanceEligibility::Blocked
        );
        assert!(assessment
            .blockers
            .contains(&IdleBlocker::GracePeriodRemaining));
        assert_eq!(
            assessment.time_until_next_transition,
            Some(Duration::from_secs(270))
        );
    }

    #[test]
    fn test_hard_gate_charging_and_battery_thresholds_for_deep_idle() {
        // Grace elapsed, but NOT charging
        let ctx_not_charging = IdleContext {
            screen: ScreenState::Off,
            screen_off_duration: Some(Duration::from_secs(400)),
            charging: false,
            battery_percent: 85,
            cpu_psi_pct: SensorReading::available(0.5),
            io_psi_pct: SensorReading::available(0.2),
            mem_psi_pct: SensorReading::available(0.1),
            thermal_celsius: SensorReading::available(32.0),
            thermal_source: None,
            stationary: true,
            user_active: false,
        };

        let assessment = IdlePolicy::evaluate(
            &ctx_not_charging,
            IdleState::IdleCandidate,
            ThermalHysteresisState::Normal,
            Duration::from_secs(300),
        );

        assert_eq!(assessment.state, IdleState::Idle);
        assert_eq!(
            assessment.standard_maintenance,
            MaintenanceEligibility::Allowed
        );
        // Heavy maintenance blocked because not charging
        assert_eq!(
            assessment.heavy_maintenance,
            MaintenanceEligibility::Blocked
        );
        assert!(assessment.blockers.contains(&IdleBlocker::NotCharging));

        // Charging, but battery = 69% (< 70%)
        let ctx_low_battery = IdleContext {
            screen: ScreenState::Off,
            screen_off_duration: Some(Duration::from_secs(400)),
            charging: true,
            battery_percent: 69,
            cpu_psi_pct: SensorReading::available(0.5),
            io_psi_pct: SensorReading::available(0.2),
            mem_psi_pct: SensorReading::available(0.1),
            thermal_celsius: SensorReading::available(32.0),
            thermal_source: None,
            stationary: true,
            user_active: false,
        };

        let assessment_low_bat = IdlePolicy::evaluate(
            &ctx_low_battery,
            IdleState::IdleCandidate,
            ThermalHysteresisState::Normal,
            Duration::from_secs(300),
        );

        assert_eq!(assessment_low_bat.state, IdleState::Idle);
        assert_eq!(
            assessment_low_bat.standard_maintenance,
            MaintenanceEligibility::Allowed
        );
        assert_eq!(
            assessment_low_bat.heavy_maintenance,
            MaintenanceEligibility::Blocked
        );
        assert!(assessment_low_bat
            .blockers
            .contains(&IdleBlocker::BatteryBelowDeepThreshold));
    }

    #[test]
    fn test_battery_critical_hard_blocks_all_maintenance() {
        let ctx = IdleContext {
            screen: ScreenState::Off,
            screen_off_duration: Some(Duration::from_secs(600)),
            charging: false,
            battery_percent: 15, // < 20%
            cpu_psi_pct: SensorReading::available(0.1),
            io_psi_pct: SensorReading::available(0.1),
            mem_psi_pct: SensorReading::available(0.1),
            thermal_celsius: SensorReading::available(30.0),
            thermal_source: None,
            stationary: true,
            user_active: false,
        };

        let assessment = IdlePolicy::evaluate(
            &ctx,
            IdleState::Idle,
            ThermalHysteresisState::Normal,
            Duration::from_secs(300),
        );

        assert_eq!(
            assessment.standard_maintenance,
            MaintenanceEligibility::Blocked
        );
        assert_eq!(
            assessment.heavy_maintenance,
            MaintenanceEligibility::Blocked
        );
        assert!(assessment.blockers.contains(&IdleBlocker::BatteryTooLow));
    }

    #[test]
    fn test_thermal_hysteresis_transitions() {
        let mut st = ThermalHysteresisState::Normal;

        // Normal @ 38.0°C
        st = ThermalHysteresisState::next_state(st, 38.0);
        assert_eq!(st, ThermalHysteresisState::Normal);

        // Rises to 42.0°C -> Warm
        st = ThermalHysteresisState::next_state(st, 42.0);
        assert_eq!(st, ThermalHysteresisState::Warm);

        // Rises to 46.0°C -> Hot (Paused)
        st = ThermalHysteresisState::next_state(st, 46.0);
        assert_eq!(st, ThermalHysteresisState::Hot);

        // Drops to 44.0°C -> MUST REMAIN Hot due to hysteresis (cooling threshold <= 40°C)
        st = ThermalHysteresisState::next_state(st, 44.0);
        assert_eq!(st, ThermalHysteresisState::Hot);

        // Drops to 41.0°C -> MUST REMAIN Hot
        st = ThermalHysteresisState::next_state(st, 41.0);
        assert_eq!(st, ThermalHysteresisState::Hot);

        // Drops to 39.5°C (<= 40.0°C) -> Recovers to Normal
        st = ThermalHysteresisState::next_state(st, 39.5);
        assert_eq!(st, ThermalHysteresisState::Normal);

        // Rises to 52.0°C -> Critical
        st = ThermalHysteresisState::next_state(st, 52.0);
        assert_eq!(st, ThermalHysteresisState::Critical);
    }

    #[test]
    fn test_fake_clock_idle_manager_state_machine_transition() {
        let fake_clock = Arc::new(FakeClock::new());
        let mut manager =
            IdleManager::new(Duration::from_secs(300), fake_clock.clone() as Arc<dyn Clock>);

        let mut ctx = IdleContext {
            screen: ScreenState::On,
            screen_off_duration: None,
            charging: true,
            battery_percent: 85,
            cpu_psi_pct: SensorReading::available(0.2),
            io_psi_pct: SensorReading::available(0.1),
            mem_psi_pct: SensorReading::available(0.1),
            thermal_celsius: SensorReading::available(35.0),
            thermal_source: None,
            stationary: true,
            user_active: false,
        };

        // 1. Initial Screen ON -> Active
        let assess = manager.update(&mut ctx);
        assert_eq!(assess.state, IdleState::Active);

        // 2. Screen turns OFF
        ctx.screen = ScreenState::Off;
        let assess = manager.update(&mut ctx);
        assert_eq!(assess.state, IdleState::IdleCandidate);
        assert_eq!(
            assess.standard_maintenance,
            MaintenanceEligibility::Blocked
        );

        // 3. Advance fake clock by 2 minutes (< 5 min)
        fake_clock.advance(Duration::from_secs(120));
        let assess = manager.update(&mut ctx);
        assert_eq!(assess.state, IdleState::IdleCandidate);
        assert_eq!(
            assess.time_until_next_transition,
            Some(Duration::from_secs(180))
        );

        // 4. Advance fake clock by another 3.5 minutes (total 5.5 min > 5 min)
        fake_clock.advance(Duration::from_secs(210));
        let assess = manager.update(&mut ctx);
        assert_eq!(assess.state, IdleState::DeepIdle);
        assert_eq!(
            assess.standard_maintenance,
            MaintenanceEligibility::Allowed
        );
        assert_eq!(assess.heavy_maintenance, MaintenanceEligibility::Allowed);

        // 5. Screen turns ON -> Immediate transition back to Active
        ctx.screen = ScreenState::On;
        manager.on_screen_state_change(ScreenState::On);
        let assess = manager.update(&mut ctx);
        assert_eq!(assess.state, IdleState::Active);
        assert_eq!(
            assess.standard_maintenance,
            MaintenanceEligibility::Blocked
        );
    }
}
