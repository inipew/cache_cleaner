use std::time::Duration;

use super::assessment::{IdleAssessment, IdleBlocker, IdlePositive};
use super::sensors::IdleContext;
use super::state::{IdleState, MaintenanceEligibility, ThermalHysteresisState};
use crate::hardware::ScreenState;

pub struct IdlePolicy;

impl IdlePolicy {
    /// Pure policy evaluation deriving score, state transitions, hard safety gates, and maintenance eligibility
    pub fn evaluate(
        ctx: &IdleContext,
        _current_state: IdleState,
        thermal_state: ThermalHysteresisState,
        grace_period: Duration,
    ) -> IdleAssessment {
        let mut score: u8 = 0;
        let mut blockers = Vec::new();
        let mut positives = Vec::new();

        // 1. Evaluate Screen & Grace Duration
        let is_screen_off = matches!(ctx.screen, ScreenState::Off);
        let screen_off_dur = ctx.screen_off_duration.unwrap_or(Duration::ZERO);
        let grace_elapsed = is_screen_off && screen_off_dur >= grace_period;

        if is_screen_off {
            score = score.saturating_add(25);
            positives.push(IdlePositive::ScreenOff);

            if grace_elapsed {
                score = score.saturating_add(10);
                positives.push(IdlePositive::GraceElapsed);
            } else {
                blockers.push(IdleBlocker::GracePeriodRemaining);
            }
        } else {
            blockers.push(IdleBlocker::ScreenOn);
        }

        // 2. Evaluate Charging & Battery Level
        if ctx.charging {
            score = score.saturating_add(15);
            positives.push(IdlePositive::Charging);
        } else {
            blockers.push(IdleBlocker::NotCharging);
        }

        if ctx.battery_percent >= 70 {
            score = score.saturating_add(10);
            positives.push(IdlePositive::HighBattery);
        } else if ctx.battery_percent < 20 {
            blockers.push(IdleBlocker::BatteryTooLow);
        } else {
            blockers.push(IdleBlocker::BatteryBelowDeepThreshold);
        }

        // 3. Evaluate CPU Pressure (PSI)
        let cpu_low = if let Some(cpu_pct) = ctx.cpu_psi_pct.value {
            if cpu_pct < 5.0 {
                score = score.saturating_add(10);
                positives.push(IdlePositive::CpuIdle);
                true
            } else {
                if cpu_pct >= 10.0 {
                    blockers.push(IdleBlocker::CpuPressureHigh);
                }
                false
            }
        } else {
            // If PSI supported but unavailable
            if ctx.cpu_psi_pct.status == super::sensors::SensorStatus::TemporarilyUnavailable {
                blockers.push(IdleBlocker::SensorUnavailable);
            }
            true // Neutral if unsupported
        };

        // 4. Evaluate I/O Pressure (PSI)
        let io_low = if let Some(io_pct) = ctx.io_psi_pct.value {
            if io_pct < 3.0 {
                score = score.saturating_add(10);
                positives.push(IdlePositive::IoIdle);
                true
            } else {
                if io_pct >= 8.0 {
                    blockers.push(IdleBlocker::IoPressureHigh);
                }
                false
            }
        } else {
            if ctx.io_psi_pct.status == super::sensors::SensorStatus::TemporarilyUnavailable {
                blockers.push(IdleBlocker::SensorUnavailable);
            }
            true
        };

        // 5. Evaluate Thermal State
        let thermal_safe = if let Some(temp) = ctx.thermal_celsius.value {
            if temp < 38.0 {
                score = score.saturating_add(10);
                positives.push(IdlePositive::ThermalSafe);
            }
            temp < 40.0
        } else {
            true
        };

        match thermal_state {
            ThermalHysteresisState::Hot => blockers.push(IdleBlocker::ThermalHot),
            ThermalHysteresisState::Critical => blockers.push(IdleBlocker::ThermalCritical),
            _ => {}
        }

        // 6. Stationary & User Activity
        if ctx.stationary && !ctx.user_active {
            score = score.saturating_add(10);
            positives.push(IdlePositive::Stationary);
        }
        if ctx.user_active {
            blockers.push(IdleBlocker::RecentActivity);
        }

        // Strictly bound score to [0..=100]
        let final_score = score.min(100);

        // 7. Determine Next State via Strict State Machine Rules
        let next_state = if !is_screen_off || ctx.user_active || thermal_state == ThermalHysteresisState::Critical {
            IdleState::Active
        } else if !grace_elapsed {
            IdleState::IdleCandidate
        } else {
            // Screen OFF and grace period satisfied
            let deep_eligible = ctx.charging
                && ctx.battery_percent >= 70
                && thermal_state == ThermalHysteresisState::Normal
                && thermal_safe
                && cpu_low
                && io_low
                && !ctx.user_active;

            if deep_eligible {
                IdleState::DeepIdle
            } else {
                IdleState::Idle
            }
        };

        // 8. Determine Hard Maintenance Gating (Eligibility)
        let standard_maintenance = if thermal_state == ThermalHysteresisState::Hot {
            MaintenanceEligibility::Paused
        } else if thermal_state == ThermalHysteresisState::Critical
            || !is_screen_off
            || !grace_elapsed
            || ctx.user_active
            || ctx.battery_percent < 20
            || blockers.contains(&IdleBlocker::CpuPressureHigh)
            || blockers.contains(&IdleBlocker::IoPressureHigh)
        {
            MaintenanceEligibility::Blocked
        } else if next_state == IdleState::Idle || next_state == IdleState::DeepIdle {
            MaintenanceEligibility::Allowed
        } else {
            MaintenanceEligibility::Blocked
        };

        let heavy_maintenance = if thermal_state == ThermalHysteresisState::Hot {
            MaintenanceEligibility::Paused
        } else if next_state == IdleState::DeepIdle
            && standard_maintenance == MaintenanceEligibility::Allowed
            && ctx.charging
            && ctx.battery_percent >= 70
            && thermal_state == ThermalHysteresisState::Normal
        {
            MaintenanceEligibility::Allowed
        } else {
            MaintenanceEligibility::Blocked
        };

        // 9. Work Budget / Rate Limiting (ops/s)
        let rate_limit_ops_per_sec = match thermal_state {
            ThermalHysteresisState::Critical | ThermalHysteresisState::Hot => 0,
            ThermalHysteresisState::Warm => 300,
            ThermalHysteresisState::Normal => {
                if blockers.contains(&IdleBlocker::IoPressureHigh) {
                    50
                } else if blockers.contains(&IdleBlocker::CpuPressureHigh) {
                    150
                } else {
                    500
                }
            }
        };

        // 10. Compute Time Until Next State Transition
        let time_until_next_transition = if next_state == IdleState::IdleCandidate {
            Some(grace_period.saturating_sub(screen_off_dur))
        } else {
            None
        };

        IdleAssessment {
            score: final_score,
            state: next_state,
            thermal_state,
            standard_maintenance,
            heavy_maintenance,
            blockers,
            positives,
            rate_limit_ops_per_sec,
            time_until_next_transition,
        }
    }
}
