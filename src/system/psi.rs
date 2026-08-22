use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct PsiMetricSample {
    pub avg10: f32,
    pub avg60: f32,
    pub avg300: f32,
    pub total_us: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PsiMetrics {
    pub some: PsiMetricSample,
    pub full: Option<PsiMetricSample>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PsiPressureLevel {
    Normal,
    Moderate,
    Critical,
}

impl std::fmt::Display for PsiPressureLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PsiPressureLevel::Normal => write!(f, "Normal (Low Stall)"),
            PsiPressureLevel::Moderate => write!(f, "Moderate (Elevated Stall)"),
            PsiPressureLevel::Critical => write!(f, "Critical (Severe Stall / Thrashing)"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PsiDiagnostics {
    pub is_supported: bool,
    pub memory_metrics: Option<PsiMetrics>,
    pub io_metrics: Option<PsiMetrics>,
    pub cpu_metrics: Option<PsiMetrics>,
    pub current_level: String,
}

/// Linux Kernel PSI Watcher and Trigger Manager
pub struct PsiWatcher {
    #[cfg(unix)]
    pub moderate_fd: Option<libc::c_int>,
    #[cfg(unix)]
    pub critical_fd: Option<libc::c_int>,
    pub last_response: Instant,
}

impl PsiWatcher {
    /// Creates and initializes kernel PSI triggers for memory stall monitoring
    pub fn create(moderate_stall_ms: u32, critical_stall_ms: u32) -> Self {
        #[cfg(unix)]
        {
            let moderate_us = (moderate_stall_ms as u64) * 1000;
            let critical_us = (critical_stall_ms as u64) * 1000;
            let window_us: u64 = 1_000_000; // 1 second window

            let moderate_fd =
                open_psi_trigger("/proc/pressure/memory", "some", moderate_us, window_us);
            let critical_fd =
                open_psi_trigger("/proc/pressure/memory", "full", critical_us, window_us);

            if moderate_fd.is_some() || critical_fd.is_some() {
                log::info!(
                    "Kernel PSI triggers registered: moderate_fd={:?}, critical_fd={:?}",
                    moderate_fd,
                    critical_fd
                );
            } else {
                log::debug!("Kernel PSI not supported or triggers could not be registered");
            }

            Self {
                moderate_fd,
                critical_fd,
                last_response: Instant::now() - std::time::Duration::from_secs(3600),
            }
        }

        #[cfg(not(unix))]
        {
            Self {
                last_response: Instant::now() - std::time::Duration::from_secs(3600),
            }
        }
    }

    #[cfg(unix)]
    #[must_use]
    pub fn get_raw_fds(&self) -> Vec<libc::c_int> {
        let mut fds = Vec::new();
        if let Some(fd) = self.moderate_fd {
            fds.push(fd);
        }
        if let Some(fd) = self.critical_fd {
            fds.push(fd);
        }
        fds
    }

    /// Checks if a given FD belongs to this watcher and whether it is critical
    #[cfg(unix)]
    #[must_use]
    pub fn identify_fd(&self, fd: libc::c_int) -> Option<PsiPressureLevel> {
        if Some(fd) == self.critical_fd {
            Some(PsiPressureLevel::Critical)
        } else if Some(fd) == self.moderate_fd {
            Some(PsiPressureLevel::Moderate)
        } else {
            None
        }
    }

    /// Determines if enough cooldown time has elapsed since the last PSI response
    #[must_use]
    pub fn can_respond(&self, cooldown_secs: u64) -> bool {
        self.last_response.elapsed() >= std::time::Duration::from_secs(cooldown_secs)
    }

    /// Records that an adaptive PSI response was executed
    pub fn record_response(&mut self) {
        self.last_response = Instant::now();
    }
}

impl Drop for PsiWatcher {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            if let Some(fd) = self.moderate_fd.take() {
                unsafe { libc::close(fd) };
            }
            if let Some(fd) = self.critical_fd.take() {
                unsafe { libc::close(fd) };
            }
        }
    }
}

/// Checks if PSI is supported by the kernel
#[must_use]
pub fn is_psi_supported() -> bool {
    Path::new("/proc/pressure/memory").exists()
}

/// Reads and parses current Memory Pressure Stall Information from `/proc/pressure/memory`
#[must_use]
pub fn read_memory_pressure() -> Option<PsiMetrics> {
    parse_psi_file("/proc/pressure/memory")
}

/// Reads and parses current I/O Pressure Stall Information from `/proc/pressure/io`
#[must_use]
pub fn read_io_pressure() -> Option<PsiMetrics> {
    parse_psi_file("/proc/pressure/io")
}

/// Reads and parses current CPU Pressure Stall Information from `/proc/pressure/cpu`
#[must_use]
pub fn read_cpu_pressure() -> Option<PsiMetrics> {
    parse_psi_file("/proc/pressure/cpu")
}


#[cfg(unix)]
use std::ffi::CString;

/// Evaluates current pressure level based on instantaneous avg10 values
#[must_use]
pub fn evaluate_current_pressure_level() -> PsiPressureLevel {
    if let Some(mem) = read_memory_pressure() {
        if let Some(full) = mem.full {
            if full.avg10 >= 5.0 {
                return PsiPressureLevel::Critical;
            }
        }
        if mem.some.avg10 >= 10.0 {
            return PsiPressureLevel::Moderate;
        }
    }
    PsiPressureLevel::Normal
}

/// Gathers comprehensive PSI diagnostics for status reporting and CLI
#[must_use]
pub fn get_psi_diagnostics() -> PsiDiagnostics {
    let supported = is_psi_supported();
    let mem = read_memory_pressure();
    let io = read_io_pressure();
    let cpu = read_cpu_pressure();
    let level = evaluate_current_pressure_level();

    PsiDiagnostics {
        is_supported: supported,
        memory_metrics: mem,
        io_metrics: io,
        cpu_metrics: cpu,
        current_level: format!("{level}"),
    }
}

/// Parses a PSI format string from `/proc/pressure/*`
#[must_use]
pub fn parse_psi_content(content: &str) -> Option<PsiMetrics> {
    let mut some_sample = None;
    let mut full_sample = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("some ") {
            some_sample = parse_psi_line(rest);
        } else if let Some(rest) = trimmed.strip_prefix("full ") {
            full_sample = parse_psi_line(rest);
        }
    }

    some_sample.map(|some| PsiMetrics {
        some,
        full: full_sample,
    })
}

fn parse_psi_file(path: &str) -> Option<PsiMetrics> {
    if !Path::new(path).exists() {
        return None;
    }
    let content = fs::read_to_string(path).ok()?;
    parse_psi_content(&content)
}

fn parse_psi_line(line: &str) -> Option<PsiMetricSample> {
    let mut sample = PsiMetricSample::default();
    let mut parsed_any = false;

    for token in line.split_whitespace() {
        if let Some((key, val)) = token.split_once('=') {
            match key {
                "avg10" => {
                    if let Ok(v) = val.parse::<f32>() {
                        sample.avg10 = v;
                        parsed_any = true;
                    }
                }
                "avg60" => {
                    if let Ok(v) = val.parse::<f32>() {
                        sample.avg60 = v;
                        parsed_any = true;
                    }
                }
                "avg300" => {
                    if let Ok(v) = val.parse::<f32>() {
                        sample.avg300 = v;
                        parsed_any = true;
                    }
                }
                "total" => {
                    if let Ok(v) = val.parse::<u64>() {
                        sample.total_us = v;
                        parsed_any = true;
                    }
                }
                _ => {}
            }
        }
    }

    if parsed_any {
        Some(sample)
    } else {
        None
    }
}

#[cfg(unix)]
fn open_psi_trigger(
    path: &str,
    trigger_type: &str,
    stall_us: u64,
    window_us: u64,
) -> Option<libc::c_int> {
    if !Path::new(path).exists() {
        return None;
    }

    let c_path = CString::new(path).ok()?;

    let fd = unsafe {
        libc::open(
            c_path.as_ptr(),
            libc::O_RDWR | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };

    if fd < 0 {
        return None;
    }

    let spec = format!("{trigger_type} {stall_us} {window_us}\0");
    let written = unsafe {
        libc::write(
            fd,
            spec.as_ptr().cast::<libc::c_void>(),
            spec.len() - 1, // Exclude trailing null byte
        )
    };

    if written <= 0 {
        unsafe { libc::close(fd) };
        return None;
    }

    Some(fd)
}

