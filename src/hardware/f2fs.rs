use std::fs;
use std::path::{Path, PathBuf};

pub struct F2fsController {
    devices: Vec<PathBuf>,
}

impl F2fsController {
    pub fn discover() -> Self {
        let mut devices = Vec::new();
        let f2fs_sysfs = Path::new("/sys/fs/f2fs");

        if f2fs_sysfs.exists() {
            if let Ok(entries) = fs::read_dir(f2fs_sysfs) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() && path.join("gc_urgent").exists() {
                        devices.push(path);
                    }
                }
            }
        }

        Self { devices }
    }

    pub fn is_available(&self) -> bool {
        !self.devices.is_empty()
    }

    /// Mode 0: Disable GC Urgent (Normal)
    /// Mode 1: GC Urgent High (Heavy I/O)
    /// Mode 2: GC Urgent Low / Background (Idle friendly, low power)
    pub fn set_gc_urgent(&self, mode: u8) {
        for dev in &self.devices {
            let gc_file = dev.join("gc_urgent");
            let mode_str = format!("{}\n", mode);
            if let Err(e) = fs::write(&gc_file, mode_str) {
                log::debug!("Failed to write gc_urgent to {}: {}", gc_file.display(), e);
            } else {
                log::info!(
                    "Set F2FS gc_urgent={} on {}",
                    mode,
                    dev.file_name().unwrap_or_default().to_string_lossy()
                );
            }
        }
    }

    /// Acquires a RAII guard that sets gc_urgent to the requested mode and restores previous states on drop
    pub fn enter_gc_urgent_scoped(&self, mode: u8) -> Option<F2fsUrgentGuard<'_>> {
        if !self.is_available() {
            return None;
        }

        let mut previous_modes = Vec::new();
        for dev in &self.devices {
            let gc_file = dev.join("gc_urgent");
            let prev = fs::read_to_string(&gc_file)
                .ok()
                .and_then(|s| s.trim().parse::<u8>().ok())
                .unwrap_or(0);
            previous_modes.push((gc_file, prev));
        }

        self.set_gc_urgent(mode);
        Some(F2fsUrgentGuard {
            _controller: self,
            previous_modes,
        })
    }
}

pub struct F2fsUrgentGuard<'a> {
    _controller: &'a F2fsController,
    previous_modes: Vec<(PathBuf, u8)>,
}

impl<'a> Drop for F2fsUrgentGuard<'a> {
    fn drop(&mut self) {
        for (gc_file, prev_mode) in &self.previous_modes {
            let mode_str = format!("{}\n", prev_mode);
            let _ = fs::write(gc_file, mode_str);
            log::debug!(
                "Restored F2FS gc_urgent={} on {}",
                prev_mode,
                gc_file.display()
            );
        }
    }
}
