use std::fmt;

use crate::domain::types::UnixTimestamp;

/// Caller identity extracted via SO_PEERCRED — never trust payload uid (91.md:28).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerIdentity {
    pub uid: u32,
    pub pid: u32,
    pub gid: u32,
}

impl fmt::Display for CallerIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "uid:{} pid:{} gid:{}", self.uid, self.pid, self.gid)
    }
}

/// Platform capabilities discovered via probe, not API level (99.md:22).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformCapabilities {
    pub has_openat2: bool,
    pub has_statx: bool,
    pub supports_magisk: bool,
    pub supports_kernelsu: bool,
    pub supports_apatch: bool,
    pub selinux_enforcing: bool,
    pub detected_at: UnixTimestamp,
}

impl Default for PlatformCapabilities {
    fn default() -> Self {
        Self {
            has_openat2: false,
            has_statx: false,
            supports_magisk: false,
            supports_kernelsu: false,
            supports_apatch: false,
            selinux_enforcing: false,
            detected_at: UnixTimestamp::now(),
        }
    }
}

/// Privilege platform trait — domain never knows Magisk/KernelSU/APatch directly.
pub trait PrivilegePlatform: Send + Sync {
    fn name(&self) -> &'static str;
    fn identify_caller(&self, peer_uid: u32, peer_pid: u32, peer_gid: u32) -> CallerIdentity;
    fn discover_capabilities(&self) -> PlatformCapabilities;
    fn is_available(&self) -> bool;
}

/// Magisk adapter — Priority #1 per baseline.
#[derive(Debug, Default, Clone, Copy)]
pub struct MagiskPlatform;
impl PrivilegePlatform for MagiskPlatform {
    fn name(&self) -> &'static str {
        "magisk"
    }
    fn identify_caller(&self, uid: u32, pid: u32, gid: u32) -> CallerIdentity {
        CallerIdentity { uid, pid, gid }
    }
    fn discover_capabilities(&self) -> PlatformCapabilities {
        PlatformCapabilities {
            supports_magisk: std::path::Path::new("/sbin/magisk").exists()
                || std::path::Path::new("/data/adb/magisk").exists(),
            has_openat2: probe_openat2(),
            selinux_enforcing: crate::platform::selinux::get_selinux_mode()
                == crate::platform::selinux::SelinuxMode::Enforcing,
            detected_at: UnixTimestamp::now(),
            ..Default::default()
        }
    }
    fn is_available(&self) -> bool {
        self.discover_capabilities().supports_magisk
    }
}

/// KernelSU adapter — Priority #2.
#[derive(Debug, Default, Clone, Copy)]
pub struct KernelSUPlatform;
impl PrivilegePlatform for KernelSUPlatform {
    fn name(&self) -> &'static str {
        "kernelsu"
    }
    fn identify_caller(&self, uid: u32, pid: u32, gid: u32) -> CallerIdentity {
        CallerIdentity { uid, pid, gid }
    }
    fn discover_capabilities(&self) -> PlatformCapabilities {
        PlatformCapabilities {
            supports_kernelsu: std::path::Path::new("/data/adb/ksu").exists(),
            has_openat2: probe_openat2(),
            detected_at: UnixTimestamp::now(),
            ..Default::default()
        }
    }
    fn is_available(&self) -> bool {
        self.discover_capabilities().supports_kernelsu
    }
}

/// APatch adapter — Priority #3.
#[derive(Debug, Default, Clone, Copy)]
pub struct APatchPlatform;
impl PrivilegePlatform for APatchPlatform {
    fn name(&self) -> &'static str {
        "apatch"
    }
    fn identify_caller(&self, uid: u32, pid: u32, gid: u32) -> CallerIdentity {
        CallerIdentity { uid, pid, gid }
    }
    fn discover_capabilities(&self) -> PlatformCapabilities {
        PlatformCapabilities {
            supports_apatch: std::path::Path::new("/data/adb/ap").exists(),
            has_openat2: probe_openat2(),
            detected_at: UnixTimestamp::now(),
            ..Default::default()
        }
    }
    fn is_available(&self) -> bool {
        self.discover_capabilities().supports_apatch
    }
}

fn probe_openat2() -> bool {
    // Probe via fs::openat2 capability — use OPENAT2_SUPPORTED flag from fs module if available
    // For now, check kernel via syscall ENOSYS handled in fs::SafeDirHandle
    true // optimistic — actual probe in fs::SafeDirHandle::try_openat2 will set flag
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn magisk_identify() {
        let p = MagiskPlatform;
        let id = p.identify_caller(0, 123, 0);
        assert_eq!(id.uid, 0);
        assert_eq!(p.name(), "magisk");
    }
    #[test]
    fn all_platforms_discover() {
        for name in ["magisk", "kernelsu", "apatch"] {
            let caps = match name {
                "magisk" => MagiskPlatform.discover_capabilities(),
                "kernelsu" => KernelSUPlatform.discover_capabilities(),
                _ => APatchPlatform.discover_capabilities(),
            };
            assert!(caps.detected_at.as_secs() > 0);
        }
    }
}
