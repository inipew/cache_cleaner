#[cfg(test)]
mod tests {
    use cache_cleaner_daemon::hardware::uevent::{get_charger_state, get_screen_state, ChargerState, ScreenState, UeventMessage};

    #[test]
    fn test_parse_power_supply_uevent() {
        let raw_payload = b"change@/devices/platform/soc/1000000.pinctrl/power_supply/battery\0ACTION=change\0DEVPATH=/devices/platform/soc/1000000.pinctrl/power_supply/battery\0SUBSYSTEM=power_supply\0POWER_SUPPLY_NAME=battery\0POWER_SUPPLY_STATUS=Charging\0POWER_SUPPLY_CAPACITY=85\0POWER_SUPPLY_TEMP=325\0";

        let event = UeventMessage::parse(raw_payload).expect("Failed to parse power_supply uevent");
        assert_eq!(event.action, "change");
        assert_eq!(event.subsystem, "power_supply");
        assert_eq!(event.devpath, "/devices/platform/soc/1000000.pinctrl/power_supply/battery");
        assert_eq!(event.properties.get("POWER_SUPPLY_STATUS").map(|s| s.as_str()), Some("Charging"));
        assert_eq!(event.properties.get("POWER_SUPPLY_CAPACITY").map(|s| s.as_str()), Some("85"));
        assert_eq!(event.properties.get("POWER_SUPPLY_TEMP").map(|s| s.as_str()), Some("325"));
    }

    #[test]
    fn test_parse_backlight_uevent() {
        let raw_payload = b"change@/devices/platform/soc/panel/backlight/panel0-backlight\0ACTION=change\0DEVPATH=/devices/platform/soc/panel/backlight/panel0-backlight\0SUBSYSTEM=backlight\0BRIGHTNESS=120\0ACTUAL_BRIGHTNESS=120\0";

        let event = UeventMessage::parse(raw_payload).expect("Failed to parse backlight uevent");
        assert_eq!(event.action, "change");
        assert_eq!(event.subsystem, "backlight");
        assert_eq!(event.properties.get("BRIGHTNESS").map(|s| s.as_str()), Some("120"));
        assert_eq!(event.properties.get("ACTUAL_BRIGHTNESS").map(|s| s.as_str()), Some("120"));
    }

    #[test]
    fn test_parse_drm_graphics_uevent() {
        let raw_payload = b"change@/devices/platform/soc/display-subsystem/drm/card0\0ACTION=change\0DEVPATH=/devices/platform/soc/display-subsystem/drm/card0\0SUBSYSTEM=drm\0";

        let event = UeventMessage::parse(raw_payload).expect("Failed to parse drm uevent");
        assert_eq!(event.action, "change");
        assert_eq!(event.subsystem, "drm");
        assert_eq!(event.devpath, "/devices/platform/soc/display-subsystem/drm/card0");
    }

    #[test]
    fn test_parse_empty_or_invalid_uevent() {
        assert!(UeventMessage::parse(b"").is_none());
        assert!(UeventMessage::parse(b"\0\0\0").is_none());
    }

    #[test]
    fn test_hardware_state_query_does_not_panic() {
        let _charger = get_charger_state();
        let _screen = get_screen_state();
        assert!(matches!(_charger, ChargerState::Charging | ChargerState::Discharging | ChargerState::Full | ChargerState::Unknown));
        assert!(matches!(_screen, ScreenState::On | ScreenState::Off | ScreenState::Unknown));
    }
}
