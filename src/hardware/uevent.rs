use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChargerState {
    Charging,
    Discharging,
    Full,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
        let (action_from_header, devpath_from_header) = if let Some((act, path)) = first_line.split_once('@') {
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

            if let Ok(supply_type) = fs::read_to_string(&type_path) {
                let type_upper = supply_type.trim().to_uppercase();

                if type_upper.contains("USB")
                    || type_upper.contains("AC")
                    || type_upper.contains("MAINS")
                    || type_upper.contains("WIRELESS")
                    || type_upper.contains("WIPOWER")
                {
                    if let Ok(online_val) = fs::read_to_string(&online_path) {
                        if online_val.trim() == "1" {
                            is_online_power_source = true;
                        }
                    }
                } else if type_upper.contains("BATTERY") || type_upper.contains("BMS") {
                    if let Ok(status_str) = fs::read_to_string(&status_path) {
                        let st = status_str.trim();
                        if st.eq_ignore_ascii_case("Charging") {
                            battery_status = ChargerState::Charging;
                        } else if st.eq_ignore_ascii_case("Full") {
                            battery_status = ChargerState::Full;
                        } else if st.eq_ignore_ascii_case("Discharging")
                            || st.eq_ignore_ascii_case("Not charging")
                        {
                            if battery_status == ChargerState::Unknown {
                                battery_status = ChargerState::Discharging;
                            }
                        }
                    }
                }
            } else {
                // Device without type file: test status and online directly
                if let Ok(status_str) = fs::read_to_string(&status_path) {
                    let st = status_str.trim();
                    if st.eq_ignore_ascii_case("Charging") || st == "1" {
                        battery_status = ChargerState::Charging;
                    } else if st.eq_ignore_ascii_case("Full") {
                        battery_status = ChargerState::Full;
                    } else if st.eq_ignore_ascii_case("Discharging") {
                        if battery_status == ChargerState::Unknown {
                            battery_status = ChargerState::Discharging;
                        }
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

    for path in &status_paths {
        if let Ok(content) = fs::read_to_string(path) {
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

/// Dynamically discovers screen backlight, framebuffers, DRM nodes, and display status
pub fn get_screen_state() -> ScreenState {
    // 1. Check all dynamic backlight devices in /sys/class/backlight/
    if let Ok(entries) = fs::read_dir("/sys/class/backlight") {
        for entry in entries.flatten() {
            let p = entry.path();
            let b_path = p.join("brightness");
            let actual_path = p.join("actual_brightness");

            // Try actual_brightness first, then brightness
            let content = fs::read_to_string(&actual_path).or_else(|_| fs::read_to_string(&b_path));
            if let Ok(val_str) = content {
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
            if name_str.contains("backlight") || name_str.contains("lcd") || name_str.contains("wled") {
                let b_path = entry.path().join("brightness");
                if let Ok(content) = fs::read_to_string(&b_path) {
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
    // 0 = FB_BLANK_UNBLANK (Screen is ON), >0 = Blanked / Screen OFF
    let fb_blank_paths = [
        "/sys/class/graphics/fb0/blank",
        "/sys/class/graphics/fb1/blank",
    ];
    for path in &fb_blank_paths {
        if let Ok(content) = fs::read_to_string(path) {
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
    // 0 = DRM_MODE_DPMS_ON, 3 = DRM_MODE_DPMS_OFF
    if let Ok(entries) = fs::read_dir("/sys/class/drm") {
        for entry in entries.flatten() {
            let dpms_path = entry.path().join("dpms");
            if let Ok(dpms_str) = fs::read_to_string(&dpms_path) {
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
    /// Opens and binds a Linux kernel NETLINK_KOBJECT_UEVENT socket
    pub fn open() -> std::io::Result<Self> {
        let sock = unsafe {
            libc::socket(
                libc::AF_NETLINK,
                libc::SOCK_DGRAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                15, // NETLINK_KOBJECT_UEVENT
            )
        };

        if sock < 0 {
            return Err(std::io::Error::last_os_error());
        }

        // Increase receive buffer size to 256KB to avoid dropping bursts of kernel events
        let rcvbuf: libc::c_int = 256 * 1024;
        unsafe {
            libc::setsockopt(
                sock,
                libc::SOL_SOCKET,
                libc::SO_RCVBUFFORCE,
                &rcvbuf as *const _ as *const libc::c_void,
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            );
            libc::setsockopt(
                sock,
                libc::SOL_SOCKET,
                libc::SO_RCVBUF,
                &rcvbuf as *const _ as *const libc::c_void,
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            );
        }

        let mut sa: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        sa.nl_family = libc::AF_NETLINK as libc::sa_family_t;
        sa.nl_groups = 1; // Multicast group 1 (kernel uevents)
        sa.nl_pid = 0;

        let res = unsafe {
            libc::bind(
                sock,
                &sa as *const libc::sockaddr_nl as *const libc::sockaddr,
                std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
            )
        };

        if res < 0 {
            unsafe { libc::close(sock) };
            return Err(std::io::Error::last_os_error());
        }

        Ok(Self { fd: sock })
    }

    /// Read all pending uevents from the netlink socket without blocking
    pub fn read_events(&self) -> Vec<UeventMessage> {
        let mut events = Vec::new();
        let mut buf = [0u8; 4096];

        loop {
            let n = unsafe {
                libc::recv(
                    self.fd,
                    buf.as_mut_ptr() as *mut libc::c_void,
                    buf.len(),
                    libc::MSG_DONTWAIT,
                )
            };

            if n <= 0 {
                break;
            }

            if let Some(event) = UeventMessage::parse(&buf[..n as usize]) {
                events.push(event);
            }
        }

        events
    }
}

#[cfg(unix)]
impl Drop for UeventSocket {
    fn drop(&mut self) {
        if self.fd >= 0 {
            unsafe { libc::close(self.fd) };
        }
    }
}
