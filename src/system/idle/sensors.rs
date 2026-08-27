use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::hardware::ScreenState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorStatus {
    Available,
    TemporarilyUnavailable,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SensorReading<T> {
    pub value: Option<T>,
    pub status: SensorStatus,
}

impl<T> SensorReading<T> {
    pub fn available(value: T) -> Self {
        Self {
            value: Some(value),
            status: SensorStatus::Available,
        }
    }

    pub fn unavailable() -> Self {
        Self {
            value: None,
            status: SensorStatus::TemporarilyUnavailable,
        }
    }

    pub fn unsupported() -> Self {
        Self {
            value: None,
            status: SensorStatus::Unsupported,
        }
    }

    #[inline]
    pub fn is_available(&self) -> bool {
        self.status == SensorStatus::Available && self.value.is_some()
    }
}

/// Normalized sensor context capturing all hardware signals at evaluation instant
#[derive(Debug, Clone)]
pub struct IdleContext {
    pub screen: ScreenState,
    pub screen_off_duration: Option<Duration>,
    pub charging: bool,
    pub battery_percent: u8,
    pub cpu_psi_pct: SensorReading<f32>,
    pub io_psi_pct: SensorReading<f32>,
    pub mem_psi_pct: SensorReading<f32>,
    pub thermal_celsius: SensorReading<f32>,
    pub thermal_source: Option<String>,
    pub stationary: bool,
    pub user_active: bool,
}

impl Default for IdleContext {
    fn default() -> Self {
        Self {
            screen: ScreenState::Unknown,
            screen_off_duration: None,
            charging: false,
            battery_percent: 50,
            cpu_psi_pct: SensorReading::unsupported(),
            io_psi_pct: SensorReading::unsupported(),
            mem_psi_pct: SensorReading::unsupported(),
            thermal_celsius: SensorReading::unsupported(),
            thermal_source: None,
            stationary: true,
            user_active: false,
        }
    }
}
