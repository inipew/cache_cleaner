use std::fs;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelinuxMode {
    Enforcing,
    Permissive,
    Disabled,
    Unknown,
}

pub fn get_selinux_mode() -> SelinuxMode {
    if let Ok(status) = fs::read_to_string("/sys/fs/selinux/enforce") {
        match status.trim() {
            "1" => return SelinuxMode::Enforcing,
            "0" => return SelinuxMode::Permissive,
            _ => {}
        }
    }

    if let Ok(output) = Command::new("getenforce").output() {
        if output.status.success() {
            let str_out = String::from_utf8_lossy(&output.stdout).to_lowercase();
            if str_out.contains("enforcing") {
                return SelinuxMode::Enforcing;
            } else if str_out.contains("permissive") {
                return SelinuxMode::Permissive;
            } else if str_out.contains("disabled") {
                return SelinuxMode::Disabled;
            }
        }
    }

    SelinuxMode::Unknown
}
