use serde::{Deserialize, Serialize};

/// Idle State Machine
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdleState {
    /// Screen ON or user active or critical load
    Active,
    /// Screen OFF, but grace period (< 5 min) not yet elapsed
    IdleCandidate,
    /// Screen OFF, grace period elapsed, thermal/load safe
    Idle,
    /// Screen OFF, charging, battery >= 70%, minimal thermal & PSI
    DeepIdle,
}

impl IdleState {
    pub fn as_str(&self) -> &'static str {
        match self {
            IdleState::Active => "ACTIVE",
            IdleState::IdleCandidate => "IDLE_CANDIDATE",
            IdleState::Idle => "IDLE",
            IdleState::DeepIdle => "DEEP_IDLE",
        }
    }
}

impl std::fmt::Display for IdleState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Orthogonal thermal safety state machine with hysteresis
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThermalHysteresisState {
    /// < 40.0°C: Full operation permitted
    Normal,
    /// 40.0°C - 44.9°C: Throttled operation
    Warm,
    /// 45.0°C - 49.9°C: Paused (Requires cooling to <= 40.0°C before returning to Normal)
    Hot,
    /// >= 50.0°C: Immediate cancellation
    Critical,
}

impl ThermalHysteresisState {
    pub fn next_state(current: Self, temp_c: f32) -> Self {
        if temp_c >= 50.0 {
            ThermalHysteresisState::Critical
        } else if temp_c >= 45.0 {
            ThermalHysteresisState::Hot
        } else if temp_c > 40.0 {
            match current {
                ThermalHysteresisState::Hot => ThermalHysteresisState::Hot, // Remain Hot until <= 40.0
                _ => ThermalHysteresisState::Warm,
            }
        } else {
            // <= 40.0°C: Fully recovers to Normal
            ThermalHysteresisState::Normal
        }
    }
}

/// Semantic maintenance authorization status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceEligibility {
    Allowed,
    Blocked,
    Paused,
}

impl MaintenanceEligibility {
    #[inline]
    pub fn is_allowed(&self) -> bool {
        matches!(self, MaintenanceEligibility::Allowed)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            MaintenanceEligibility::Allowed => "ELIGIBLE",
            MaintenanceEligibility::Blocked => "BLOCKED",
            MaintenanceEligibility::Paused => "PAUSED",
        }
    }
}

impl std::fmt::Display for MaintenanceEligibility {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}
