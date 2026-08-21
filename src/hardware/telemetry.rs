use crate::hardware::thermal::{read_thermal, ThermalReport};
use crate::hardware::uevent::{get_charger_state, get_screen_state, ChargerState, ScreenState};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceEnvironmentSnapshot {
    pub screen: ScreenState,
    pub charger: ChargerState,
    pub soc_temp_c: f32,
    pub battery_temp_c: f32,
    pub is_charging: bool,
    pub is_screen_off: bool,
}

impl DeviceEnvironmentSnapshot {
    pub fn capture() -> Self {
        let screen = get_screen_state();
        let charger = get_charger_state();
        let thermal: ThermalReport = read_thermal();

        let is_charging = matches!(charger, ChargerState::Charging | ChargerState::Full);
        let is_screen_off = matches!(screen, ScreenState::Off);

        Self {
            screen,
            charger,
            soc_temp_c: thermal.max_soc_temp_c,
            battery_temp_c: thermal.battery_temp_c,
            is_charging,
            is_screen_off,
        }
    }
}
