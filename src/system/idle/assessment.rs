use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::state::{IdleState, MaintenanceEligibility, ThermalHysteresisState};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdleBlocker {
    ScreenOn,
    GracePeriodRemaining,
    NotCharging,
    BatteryTooLow,
    BatteryBelowDeepThreshold,
    ThermalHot,
    ThermalCritical,
    CpuPressureHigh,
    IoPressureHigh,
    RecentActivity,
    SensorUnavailable,
}

impl IdleBlocker {
    pub fn description(&self) -> &'static str {
        match self {
            IdleBlocker::ScreenOn => "Screen is ON",
            IdleBlocker::GracePeriodRemaining => "Screen-off grace period active",
            IdleBlocker::NotCharging => "Device is not charging",
            IdleBlocker::BatteryTooLow => "Battery is below 20%",
            IdleBlocker::BatteryBelowDeepThreshold => "Battery is below 70% for deep maintenance",
            IdleBlocker::ThermalHot => "Thermal zone is hot (>=45°C, paused)",
            IdleBlocker::ThermalCritical => "Thermal zone is critical (>=50°C)",
            IdleBlocker::CpuPressureHigh => "CPU pressure exceeds threshold",
            IdleBlocker::IoPressureHigh => "I/O pressure exceeds threshold",
            IdleBlocker::RecentActivity => "Recent user activity detected",
            IdleBlocker::SensorUnavailable => "Critical system sensor unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdlePositive {
    ScreenOff,
    GraceElapsed,
    Charging,
    HighBattery,
    CpuIdle,
    IoIdle,
    ThermalSafe,
    Stationary,
}

impl IdlePositive {
    pub fn description(&self) -> &'static str {
        match self {
            IdlePositive::ScreenOff => "+25 Screen OFF",
            IdlePositive::GraceElapsed => "+10 Grace period satisfied",
            IdlePositive::Charging => "+15 Connected to charger",
            IdlePositive::HighBattery => "+10 Battery level >= 70%",
            IdlePositive::CpuIdle => "+10 CPU PSI is low (<5%)",
            IdlePositive::IoIdle => "+10 I/O PSI is low (<3%)",
            IdlePositive::ThermalSafe => "+10 Device temperature is cool (<38°C)",
            IdlePositive::Stationary => "+10 Device is stationary with no wakeups",
        }
    }
}

/// Comprehensive outcome of an Idle Policy evaluation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdleAssessment {
    pub score: u8, // Strictly 0..=100
    pub state: IdleState,
    pub thermal_state: ThermalHysteresisState,
    pub standard_maintenance: MaintenanceEligibility,
    pub heavy_maintenance: MaintenanceEligibility,
    pub blockers: Vec<IdleBlocker>,
    pub positives: Vec<IdlePositive>,
    pub rate_limit_ops_per_sec: u32,
    pub time_until_next_transition: Option<Duration>,
}
