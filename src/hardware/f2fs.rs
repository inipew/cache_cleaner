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

    /// Acquires a RAII guard that sets gc_urgent to the requested mode and reverts to 0 on drop
    pub fn enter_gc_urgent_scoped(&self, mode: u8) -> Option<F2fsUrgentGuard<'_>> {
        if !self.is_available() {
            return None;
        }
        self.set_gc_urgent(mode);
        Some(F2fsUrgentGuard { controller: self })
    }
}

pub struct F2fsUrgentGuard<'a> {
    controller: &'a F2fsController,
}

impl<'a> Drop for F2fsUrgentGuard<'a> {
    fn drop(&mut self) {
        self.controller.set_gc_urgent(0);
    }
}

