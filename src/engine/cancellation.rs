use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelReason {
    ScreenOn,
    UserActivity,
    ThermalCritical,
    BatteryLow,
    Unplugged,
    Shutdown,
    Timeout,
    ManualCancel,
}

impl CancelReason {
    fn to_code(self) -> u8 {
        match self {
            CancelReason::ScreenOn => 1,
            CancelReason::UserActivity => 2,
            CancelReason::ThermalCritical => 3,
            CancelReason::BatteryLow => 4,
            CancelReason::Unplugged => 5,
            CancelReason::Shutdown => 6,
            CancelReason::Timeout => 7,
            CancelReason::ManualCancel => 8,
        }
    }

    fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(CancelReason::ScreenOn),
            2 => Some(CancelReason::UserActivity),
            3 => Some(CancelReason::ThermalCritical),
            4 => Some(CancelReason::BatteryLow),
            5 => Some(CancelReason::Unplugged),
            6 => Some(CancelReason::Shutdown),
            7 => Some(CancelReason::Timeout),
            8 => Some(CancelReason::ManualCancel),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            CancelReason::ScreenOn => "ScreenOn",
            CancelReason::UserActivity => "UserActivity",
            CancelReason::ThermalCritical => "ThermalCritical",
            CancelReason::BatteryLow => "BatteryLow",
            CancelReason::Unplugged => "Unplugged",
            CancelReason::Shutdown => "Shutdown",
            CancelReason::Timeout => "Timeout",
            CancelReason::ManualCancel => "ManualCancel",
        }
    }
}

impl std::fmt::Display for CancelReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    ThermalHot,
    CpuPressureHigh,
    IoPressureHigh,
}

#[derive(Debug, Clone)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
    reason_code: Arc<AtomicU8>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            reason_code: Arc::new(AtomicU8::new(0)),
        }
    }

    #[inline]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// Cancel with a specific reason. The first cancellation reason wins atomically.
    pub fn cancel_with_reason(&self, reason: CancelReason) {
        self.cancelled.store(true, Ordering::SeqCst);
        let _ = self.reason_code.compare_exchange(
            0,
            reason.to_code(),
            Ordering::SeqCst,
            Ordering::Relaxed,
        );
    }

    /// Generic cancel defaulting to ManualCancel if no reason was previously stored
    pub fn cancel(&self) {
        self.cancel_with_reason(CancelReason::ManualCancel);
    }

    pub fn get_cancel_reason(&self) -> Option<CancelReason> {
        CancelReason::from_code(self.reason_code.load(Ordering::SeqCst))
    }

    pub fn reset(&self) {
        self.cancelled.store(false, Ordering::SeqCst);
        self.reason_code.store(0, Ordering::SeqCst);
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}
