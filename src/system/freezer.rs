use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FreezerState {
    Frozen,
    Thawed,
    Freezing,
    Unsupported,
}

impl std::fmt::Display for FreezerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FreezerState::Frozen => write!(f, "Frozen"),
            FreezerState::Thawed => write!(f, "Thawed / Active"),
            FreezerState::Freezing => write!(f, "Freezing"),
            FreezerState::Unsupported => write!(f, "Unsupported / Unknown"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FreezerDiagnostics {
    pub is_cached_apps_freezer_enabled: bool,
    pub freezer_cgroup_version: String,
    pub total_frozen_uids_count: usize,
    pub frozen_uids: Vec<u32>,
}

/// Detects if Cgroup Freezer is available and which version is active ("v2", "v1", or "none")
pub fn detect_freezer_version() -> &'static str {
    // 1. Check Cgroup v2 (Android 12+)
    if Path::new("/sys/fs/cgroup/cgroup.freeze").exists() {
        return "v2";
    }

    if let Ok(entries) = fs::read_dir("/sys/fs/cgroup") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("uid_") && entry.path().join("cgroup.freeze").exists() {
                return "v2";
            }
        }
    }

    // 2. Check Cgroup v1 (Android 11 / Legacy)
    if Path::new("/dev/freezer").exists() {
        return "v1";
    }

    "none"
}

/// Returns whether the kernel and ROM support process/UID freezing
#[allow(dead_code)]
pub fn is_freezer_supported() -> bool {
    detect_freezer_version() != "none"
}

/// Checks if Android Cached Apps Freezer is enabled via system properties or active cgroups
pub fn is_cached_apps_freezer_enabled() -> bool {
    // Check Android system properties
    let freezer_prop = crate::platform::android_prop::get_property(
        "persist.sys.cached_apps_freezer",
    )
    .or_else(|| crate::platform::android_prop::get_property("ro.config.cached_apps_freezer"));

    if let Some(val) = freezer_prop {
        let v = val.to_lowercase();
        if v == "true" || v == "1" || v == "device_default" || v == "force_frozen" {
            return true;
        } else if v == "false" || v == "0" || v == "disabled" {
            return false;
        }
    }

    // Fallback: If any UID is currently frozen in sysfs, freezer is clearly enabled
    !enumerate_frozen_uids().is_empty()
}

/// Checks the freezer state for a specific Linux UID (e.g. 10000..19999 for Android apps)
#[allow(dead_code)]
#[must_use]
pub fn get_uid_freezer_state(uid: u32) -> FreezerState {
    let version = detect_freezer_version();

    if version == "v2" {
        let uid_cgroup = PathBuf::from(format!("/sys/fs/cgroup/uid_{uid}"));
        let freeze_file = uid_cgroup.join("cgroup.freeze");
        let events_file = uid_cgroup.join("cgroup.events");

        if events_file.exists() {
            if let Ok(content) = fs::read_to_string(&events_file) {
                for line in content.lines() {
                    let trimmed = line.trim();
                    if trimmed == "frozen 1" {
                        return FreezerState::Frozen;
                    } else if trimmed == "frozen 0" {
                        return FreezerState::Thawed;
                    }
                }
            }
        }

        if freeze_file.exists() {
            if let Ok(content) = fs::read_to_string(&freeze_file) {
                if content.trim() == "1" {
                    return FreezerState::Frozen;
                } else if content.trim() == "0" {
                    return FreezerState::Thawed;
                }
            }
        }
    } else if version == "v1" {
        // Cgroup v1 check: inspect /dev/freezer/frozen/tasks or freezer.state
        let v1_frozen_procs = Path::new("/dev/freezer/frozen/cgroup.procs");
        let v1_frozen_tasks = Path::new("/dev/freezer/frozen/tasks");
        let v1_state = Path::new("/dev/freezer/freezer.state");

        if v1_state.exists() {
            if let Ok(content) = fs::read_to_string(v1_state) {
                let state_str = content.trim();
                if state_str == "FROZEN" {
                    return FreezerState::Frozen;
                } else if state_str == "FREEZING" {
                    return FreezerState::Freezing;
                } else if state_str == "THAWED" {
                    return FreezerState::Thawed;
                }
            }
        }

        if v1_frozen_procs.exists() || v1_frozen_tasks.exists() {
            // Check if any PID for this UID is in the frozen group
            let frozen_pids = read_pids_from_file(v1_frozen_procs)
                .or_else(|| read_pids_from_file(v1_frozen_tasks))
                .unwrap_or_default();

            for pid in frozen_pids {
                if get_uid_of_pid(pid) == Some(uid) {
                    return FreezerState::Frozen;
                }
            }
        }
    }

    FreezerState::Unsupported
}

/// Checks the freezer state for a specific process PID
#[allow(dead_code)]
#[must_use]
pub fn get_pid_freezer_state(pid: u32) -> FreezerState {
    let version = detect_freezer_version();

    if version == "v2" {
        // Check per-PID cgroup slice in /sys/fs/cgroup/
        let pid_events = PathBuf::from(format!("/sys/fs/cgroup/pid_{pid}/cgroup.events"));

        if pid_events.exists() {
            if let Ok(content) = fs::read_to_string(&pid_events) {
                if content.contains("frozen 1") {
                    return FreezerState::Frozen;
                } else if content.contains("frozen 0") {
                    return FreezerState::Thawed;
                }
            }
        }

        // Also check if its parent UID cgroup is frozen
        if let Some(uid) = get_uid_of_pid(pid) {
            return get_uid_freezer_state(uid);
        }
    } else if version == "v1" {
        if let Some(uid) = get_uid_of_pid(pid) {
            return get_uid_freezer_state(uid);
        }
    }

    FreezerState::Unsupported
}

/// Enumerates all Android app UIDs that are currently in FROZEN state.
/// This allows O(1) membership lookups during fast directory traversal.
#[must_use]
pub fn enumerate_frozen_uids() -> HashSet<u32> {
    let mut frozen = HashSet::new();
    let version = detect_freezer_version();

    if version == "v2" {
        // In Cgroup v2, iterate through /sys/fs/cgroup/uid_<uid>
        if let Ok(entries) = fs::read_dir("/sys/fs/cgroup") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if let Some(uid_str) = name_str.strip_prefix("uid_") {
                    if let Ok(uid) = uid_str.parse::<u32>() {
                        let path = entry.path();
                        let events = path.join("cgroup.events");
                        let freeze = path.join("cgroup.freeze");

                        let mut is_frozen = false;
                        let mut buf = [0u8; 64];
                        if events.exists() {
                            if let Some(content) = crate::util::read_file_to_buf(&events, &mut buf)
                            {
                                if content.contains("frozen 1") {
                                    is_frozen = true;
                                }
                            }
                        } else if freeze.exists() {
                            if let Some(content) = crate::util::read_file_to_buf(&freeze, &mut buf)
                            {
                                if content.trim() == "1" {
                                    is_frozen = true;
                                }
                            }
                        }

                        if is_frozen {
                            frozen.insert(uid);
                        }
                    }
                }
            }
        }
    } else if version == "v1" {
        let v1_frozen_procs = Path::new("/dev/freezer/frozen/cgroup.procs");
        let v1_frozen_tasks = Path::new("/dev/freezer/frozen/tasks");

        let pids = read_pids_from_file(v1_frozen_procs)
            .or_else(|| read_pids_from_file(v1_frozen_tasks))
            .unwrap_or_default();

        for pid in pids {
            if let Some(uid) = get_uid_of_pid(pid) {
                frozen.insert(uid);
            }
        }
    }

    frozen.shrink_to_fit();
    frozen
}

/// Gathers comprehensive Freezer diagnostics for status reporting and CLI
#[must_use]
pub fn get_freezer_diagnostics() -> FreezerDiagnostics {
    let version = detect_freezer_version();
    let is_enabled = is_cached_apps_freezer_enabled();
    let frozen_set = enumerate_frozen_uids();
    let mut frozen_uids: Vec<u32> = frozen_set.into_iter().collect();
    frozen_uids.sort_unstable();

    FreezerDiagnostics {
        is_cached_apps_freezer_enabled: is_enabled,
        freezer_cgroup_version: version.to_string(),
        total_frozen_uids_count: frozen_uids.len(),
        frozen_uids,
    }
}

fn read_pids_from_file(path: &Path) -> Option<Vec<u32>> {
    if !path.exists() {
        return None;
    }

    if let Ok(content) = fs::read_to_string(path) {
        let pids: Vec<u32> = content
            .lines()
            .filter_map(|line| line.trim().parse::<u32>().ok())
            .collect();
        return Some(pids);
    }

    None
}

fn get_uid_of_pid(pid: u32) -> Option<u32> {
    let status_path = format!("/proc/{pid}/status");
    if let Ok(content) = fs::read_to_string(&status_path) {
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("Uid:") {
                let mut parts = rest.split_ascii_whitespace();
                if let Some(uid_str) = parts.next() {
                    return uid_str.parse::<u32>().ok();
                }
            }
        }
    }
    None
}

