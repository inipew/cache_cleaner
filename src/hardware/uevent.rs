use crate::util::read_file_to_buf;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChargerState {
    Charging,
    Discharging,
    Full,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScreenState {
    On,
    Off,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UeventMessage {
    pub action: String,
    pub devpath: String,
    pub subsystem: String,
    pub properties: HashMap<String, String>,
}

impl UeventMessage {
    /// Parse raw uevent datagram buffer containing NUL-separated key-value strings
    pub fn parse(buffer: &[u8]) -> Option<Self> {
        let text = match std::str::from_utf8(buffer) {
            Ok(s) => s,
            Err(_) => return None,
        };

        let mut lines = text.split('\0').filter(|s| !s.is_empty());
        let first_line = lines.next()?;

        // First line format is typically: action@devpath (e.g. change@/devices/.../power_supply/battery)
        let (action_from_header, devpath_from_header) =
            if let Some((act, path)) = first_line.split_once('@') {
                (act.to_string(), path.to_string())
            } else {
                (String::new(), String::new())
            };

        let mut properties = HashMap::new();
        let mut action = action_from_header;
        let mut devpath = devpath_from_header;
        let mut subsystem = String::new();

        for line in lines {
            if let Some((k, v)) = line.split_once('=') {
                match k {
                    "ACTION" => action = v.to_string(),
                    "DEVPATH" => devpath = v.to_string(),
                    "SUBSYSTEM" => subsystem = v.to_string(),
                    _ => {
                        properties.insert(k.to_string(), v.to_string());
                    }
                }
            }
        }

        if action.is_empty() && subsystem.is_empty() && devpath.is_empty() {
            return None;
        }

        Some(Self {
            action,
            devpath,
            subsystem,
            properties,
        })
    }
}

/// Dynamically discovers power supply devices and determines accurate charger state
pub fn get_charger_state() -> ChargerState {
    let power_supply_dir = Path::new("/sys/class/power_supply");
    if !power_supply_dir.exists() {
        return get_charger_state_fallback();
    }

    let mut is_online_power_source = false;
    let mut battery_status = ChargerState::Unknown;
    let mut buf = [0u8; 64];

    if let Ok(entries) = fs::read_dir(power_supply_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            // Check if this node is a power input (USB, AC, Mains, Wireless, etc.)
            let type_path = path.join("type");
            let online_path = path.join("online");
            let status_path = path.join("status");

            if let Some(supply_type) = read_file_to_buf(&type_path, &mut buf) {
                let type_upper = supply_type.trim().to_uppercase();

                if type_upper.contains("USB")
                    || type_upper.contains("AC")
                    || type_upper.contains("MAINS")
                    || type_upper.contains("WIRELESS")
                    || type_upper.contains("WIPOWER")
                {
                    let mut on_buf = [0u8; 16];
                    if let Some(online_val) = read_file_to_buf(&online_path, &mut on_buf) {
                        if online_val.trim() == "1" {
                            is_online_power_source = true;
                        }
                    }
                } else if type_upper.contains("BATTERY") || type_upper.contains("BMS") {
                    let mut st_buf = [0u8; 32];
                    if let Some(status_str) = read_file_to_buf(&status_path, &mut st_buf) {
                        let st = status_str.trim();
                        if st.eq_ignore_ascii_case("Charging") {
                            battery_status = ChargerState::Charging;
                        } else if st.eq_ignore_ascii_case("Full") {
                            battery_status = ChargerState::Full;
                        } else if (st.eq_ignore_ascii_case("Discharging")
                            || st.eq_ignore_ascii_case("Not charging"))
                            && battery_status == ChargerState::Unknown
                        {
                            battery_status = ChargerState::Discharging;
                        }
                    }
                }
            } else {
                // Device without type file: test status and online directly
                let mut st_buf = [0u8; 32];
                if let Some(status_str) = read_file_to_buf(&status_path, &mut st_buf) {
                    let st = status_str.trim();
                    if st.eq_ignore_ascii_case("Charging") || st == "1" {
                        battery_status = ChargerState::Charging;
                    } else if st.eq_ignore_ascii_case("Full") {
                        battery_status = ChargerState::Full;
                    } else if st.eq_ignore_ascii_case("Discharging")
                        && battery_status == ChargerState::Unknown
                    {
                        battery_status = ChargerState::Discharging;
                    }
                }
            }
        }
    }

    if is_online_power_source {
        if battery_status == ChargerState::Full {
            ChargerState::Full
        } else {
            ChargerState::Charging
        }
    } else if battery_status != ChargerState::Unknown {
        battery_status
    } else {
        get_charger_state_fallback()
    }
}

fn get_charger_state_fallback() -> ChargerState {
    let status_paths = [
        "/sys/class/power_supply/battery/status",
        "/sys/class/power_supply/bms/status",
        "/sys/class/power_supply/usb/online",
        "/sys/class/power_supply/ac/online",
        "/sys/class/power_supply/main/online",
    ];

    let mut buf = [0u8; 32];
    for path in &status_paths {
        if let Some(content) = read_file_to_buf(Path::new(path), &mut buf) {
            let trimmed = content.trim();
            if trimmed.eq_ignore_ascii_case("Charging") || trimmed == "1" {
                return ChargerState::Charging;
            } else if trimmed.eq_ignore_ascii_case("Discharging") || trimmed == "0" {
                return ChargerState::Discharging;
            } else if trimmed.eq_ignore_ascii_case("Full") {
                return ChargerState::Full;
            }
        }
    }

    ChargerState::Unknown
}

/// Dynamically reads battery capacity percentage (0..=100) from Linux/Android sysfs
pub fn get_battery_percent() -> Option<u8> {
    let mut buf = [0u8; 16];
    let capacity_paths = [
        "/sys/class/power_supply/battery/capacity",
        "/sys/class/power_supply/bms/capacity",
        "/sys/class/power_supply/qcom-battery/capacity",
    ];

    for path in &capacity_paths {
        if let Some(content) = read_file_to_buf(Path::new(path), &mut buf) {
            if let Ok(pct) = content.trim().parse::<u8>() {
                if pct <= 100 {
                    return Some(pct);
                }
            }
        }
    }

    // Scan dynamic /sys/class/power_supply/*/capacity
    if let Ok(entries) = fs::read_dir("/sys/class/power_supply") {
        for entry in entries.flatten() {
            let cap_path = entry.path().join("capacity");
            if let Some(content) = read_file_to_buf(&cap_path, &mut buf) {
                if let Ok(pct) = content.trim().parse::<u8>() {
                    if pct <= 100 {
                        return Some(pct);
                    }
                }
            }
        }
    }

    None
}

/// Dynamically discovers screen backlight, framebuffers, DRM nodes, and display status
pub fn get_screen_state() -> ScreenState {
    let mut buf = [0u8; 32];

    // 1. Check all dynamic backlight devices in /sys/class/backlight/
    if let Ok(entries) = fs::read_dir("/sys/class/backlight") {
        for entry in entries.flatten() {
            let p = entry.path();
            let b_path = p.join("brightness");
            let actual_path = p.join("actual_brightness");

            // Try actual_brightness first, then brightness
            let content = if let Some(c) = read_file_to_buf(&actual_path, &mut buf) {
                Some(c)
            } else {
                read_file_to_buf(&b_path, &mut buf)
            };
            if let Some(val_str) = content {
                if let Ok(brightness) = val_str.trim().parse::<u32>() {
                    return if brightness > 0 {
                        ScreenState::On
                    } else {
                        ScreenState::Off
                    };
                }
            }
        }
    }

    // 2. Check LED backlight paths
    if let Ok(entries) = fs::read_dir("/sys/class/leds") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.contains("backlight")
                || name_str.contains("lcd")
                || name_str.contains("wled")
            {
                let b_path = entry.path().join("brightness");
                if let Some(content) = read_file_to_buf(&b_path, &mut buf) {
                    if let Ok(brightness) = content.trim().parse::<u32>() {
                        return if brightness > 0 {
                            ScreenState::On
                        } else {
                            ScreenState::Off
                        };
                    }
                }
            }
        }
    }

    // 3. Check Framebuffer blank status (/sys/class/graphics/fb0/blank)
    let fb_blank_paths = [
        "/sys/class/graphics/fb0/blank",
        "/sys/class/graphics/fb1/blank",
    ];
    for path in &fb_blank_paths {
        if let Some(content) = read_file_to_buf(Path::new(path), &mut buf) {
            if let Ok(blank_val) = content.trim().parse::<u32>() {
                return if blank_val == 0 {
                    ScreenState::On
                } else {
                    ScreenState::Off
                };
            }
        }
    }

    // 4. Check DRM DPMS status (/sys/class/drm/card0-*/dpms)
    if let Ok(entries) = fs::read_dir("/sys/class/drm") {
        for entry in entries.flatten() {
            let dpms_path = entry.path().join("dpms");
            if let Some(dpms_str) = read_file_to_buf(&dpms_path, &mut buf) {
                let trimmed = dpms_str.trim();
                if trimmed == "0" || trimmed.eq_ignore_ascii_case("On") {
                    return ScreenState::On;
                } else if trimmed == "3" || trimmed.eq_ignore_ascii_case("Off") {
                    return ScreenState::Off;
                }
            }
        }
    }

    // 5. Fallback: check dumpsys display & dumpsys power on Android
    #[cfg(unix)]
    {
        if let Ok(output) = std::process::Command::new("dumpsys")
            .args(["display"])
            .output()
        {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                if text.contains("mScreenState=ON")
                    || text.contains("state=ON")
                    || text.contains("Display State: ON")
                    || text.contains("Display Power: state=ON")
                {
                    return ScreenState::On;
                } else if text.contains("mScreenState=OFF")
                    || text.contains("state=OFF")
                    || text.contains("Display State: OFF")
                    || text.contains("Display Power: state=OFF")
                {
                    return ScreenState::Off;
                }
            }
        }
    }

    ScreenState::Unknown
}

#[cfg(unix)]
pub struct UeventSocket {
    pub fd: std::os::unix::io::RawFd,
}

#[cfg(unix)]
impl UeventSocket {
    /// Opens an AF_NETLINK socket subscribed to kernel uevent broadcast group
    pub fn open() -> std::io::Result<Self> {
        let fd = unsafe {
            libc::socket(
                libc::AF_NETLINK,
                libc::SOCK_RAW | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
                libc::NETLINK_KOBJECT_UEVENT,
            )
        };

        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }

        let mut sa: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        sa.nl_family = libc::AF_NETLINK as libc::sa_family_t;
        sa.nl_pid = unsafe { libc::getpid() as u32 };
        sa.nl_groups = 1; // NETLINK_KOBJECT_UEVENT group mask

        let bind_res = unsafe {
            libc::bind(
                fd,
                &sa as *const libc::sockaddr_nl as *const libc::sockaddr,
                std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
            )
        };

        if bind_res < 0 {
            unsafe { libc::close(fd) };
            return Err(std::io::Error::last_os_error());
        }

        Ok(Self { fd })
    }

    /// Read pending uevent datagram from netlink socket
    pub fn read_event(&self) -> Option<UeventMessage> {
        let mut buffer = [0u8; 4096];
        let n = unsafe {
            libc::recv(
                self.fd,
                buffer.as_mut_ptr().cast::<libc::c_void>(),
                buffer.len(),
                libc::MSG_DONTWAIT,
            )
        };

        if n > 0 {
            usize::try_from(n).ok().and_then(|len| UeventMessage::parse(&buffer[..len]))
        } else {
            None
        }
    }


    /// Read all pending uevent datagrams from netlink socket
    pub fn read_events(&self) -> Vec<UeventMessage> {
        let mut events = Vec::new();
        while let Some(ev) = self.read_event() {
            events.push(ev);
        }
        events
    }
}

#[cfg(unix)]
impl Drop for UeventSocket {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.fd);
        }
    }
}
