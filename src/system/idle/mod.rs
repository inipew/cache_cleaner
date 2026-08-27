pub mod assessment;
pub mod policy;
pub mod sensors;
pub mod state;

pub use assessment::{IdleAssessment, IdleBlocker, IdlePositive};
pub use policy::IdlePolicy;
pub use sensors::{IdleContext, SensorReading, SensorStatus};
pub use state::{IdleState, MaintenanceEligibility, ThermalHysteresisState};

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::hardware::ScreenState;
use crate::util::Clock;

/// Stateful Idle Manager tracking state machine transitions and thermal hysteresis
pub struct IdleManager {
    state: IdleState,
    candidate_since: Option<Instant>,
    last_activity: Option<Instant>,
    thermal_state: ThermalHysteresisState,
    grace_period: Duration,
    clock: Arc<dyn Clock>,
}

impl IdleManager {
    pub const DEFAULT_GRACE_PERIOD: Duration = Duration::from_secs(300); // 5 minutes

    pub fn new(grace_period: Duration, clock: Arc<dyn Clock>) -> Self {
        Self {
            state: IdleState::Active,
            candidate_since: None,
            last_activity: None,
            thermal_state: ThermalHysteresisState::Normal,
            grace_period,
            clock,
        }
    }

    /// Primary event-driven update transitioning the state machine
    pub fn update(&mut self, ctx: &mut IdleContext) -> IdleAssessment {
        let now = self.clock.now();

        // Screen state tracking for time-based grace period
        match ctx.screen {
            ScreenState::Off => {
                if self.candidate_since.is_none() {
                    self.candidate_since = Some(now);
                }
            }
            ScreenState::On => {
                self.candidate_since = None;
            }
            ScreenState::Unknown => {}
        }

        if let Some(since) = self.candidate_since {
            ctx.screen_off_duration = Some(now.saturating_duration_since(since));
        }

        // Thermal hysteresis transition
        if let Some(temp) = ctx.thermal_celsius.value {
            self.thermal_state = ThermalHysteresisState::next_state(self.thermal_state, temp);
        }

        let assessment = IdlePolicy::evaluate(ctx, self.state, self.thermal_state, self.grace_period);
        self.state = assessment.state;
        assessment
    }

    /// Read-only snapshot evaluation without state mutation
    pub fn get_assessment(&self, ctx: &IdleContext) -> IdleAssessment {
        IdlePolicy::evaluate(ctx, self.state, self.thermal_state, self.grace_period)
    }

    pub fn on_screen_state_change(&mut self, new_screen: ScreenState) {
        let now = self.clock.now();
        match new_screen {
            ScreenState::Off => {
                if self.candidate_since.is_none() {
                    self.candidate_since = Some(now);
                    log::info!("[IDLE] Screen OFF detected. Entered IdleCandidate grace period ({:?})", self.grace_period);
                }
            }
            ScreenState::On => {
                self.candidate_since = None;
                self.state = IdleState::Active;
                log::info!("[IDLE] Screen ON detected. Immediate transition to ACTIVE.");
            }
            ScreenState::Unknown => {}
        }
    }

    pub fn on_thermal_update(&mut self, temp_c: f32) -> ThermalHysteresisState {
        let next = ThermalHysteresisState::next_state(self.thermal_state, temp_c);
        if next != self.thermal_state {
            log::info!("[IDLE] Thermal state transition: {:?} -> {:?} (temp: {:.1}°C)", self.thermal_state, next, temp_c);
            self.thermal_state = next;
        }
        self.thermal_state
    }

    pub fn on_user_activity(&mut self) {
        self.last_activity = Some(self.clock.now());
        self.state = IdleState::Active;
    }

    #[inline]
    pub fn state(&self) -> IdleState {
        self.state
    }

    #[inline]
    pub fn thermal_state(&self) -> ThermalHysteresisState {
        self.thermal_state
    }
}
