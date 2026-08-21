use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

use crate::util::read_file_to_buf;

#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessMetrics {
    pub cpu_usage_pct: f32,
    pub vm_size_bytes: u64,
    pub rss_bytes: u64,
    pub pss_bytes: u64,
}

pub struct CpuTracker {
    last_ticks: u64,
    last_sample: Instant,
    clk_tck: f32,
}

impl Default for CpuTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl CpuTracker {
    pub fn new() -> Self {
        let clk_tck = {
            #[cfg(unix)]
            unsafe {
                let t = libc::sysconf(libc::_SC_CLK_TCK);
                if t > 0 {
                    t as f32
                } else {
                    100.0
                }
            }
            #[cfg(not(unix))]
            {
                100.0
            }
        };

        let mut tracker = Self {
            last_ticks: 0,
            last_sample: Instant::now(),
            clk_tck,
        };
        tracker.last_ticks = read_cpu_ticks(None);
        tracker
    }

    pub fn sample_cpu_pct(&mut self) -> f32 {
        let current_ticks = read_cpu_ticks(None);
        let elapsed = self.last_sample.elapsed().as_secs_f32();

        if elapsed < 0.05 {
            return 0.0;
        }

        let delta_ticks = current_ticks.saturating_sub(self.last_ticks);
        self.last_ticks = current_ticks;
        self.last_sample = Instant::now();

        let cpu_pct = (delta_ticks as f32 / self.clk_tck) / elapsed * 100.0;
        cpu_pct.max(0.0)
    }
}

static CPU_TRACKER: Mutex<Option<CpuTracker>> = Mutex::new(None);

/// Reads memory (Total/VmSize, RSS, PSS) and instantaneous CPU% for the current daemon process.
pub fn get_process_metrics() -> ProcessMetrics {
    let mut cpu_pct = 0.0;
    if let Ok(mut guard) = CPU_TRACKER.lock() {
        if guard.is_none() {
            *guard = Some(CpuTracker::new());
        }
        if let Some(ref mut tracker) = *guard {
            cpu_pct = tracker.sample_cpu_pct();
        }
    }

    let (vm_size_bytes, mut rss_bytes) = read_proc_status_memory(None);
    let pss_bytes = read_proc_smaps_rollup_pss(None, &mut rss_bytes);

    ProcessMetrics {
        cpu_usage_pct: cpu_pct,
        vm_size_bytes,
        rss_bytes,
        pss_bytes,
    }
}

/// Reads memory (Total/VmSize, RSS, PSS) and CPU ticks for an arbitrary PID.
pub fn get_process_metrics_for_pid(pid: u32) -> ProcessMetrics {
    let (vm_size_bytes, mut rss_bytes) = read_proc_status_memory(Some(pid));
    let pss_bytes = read_proc_smaps_rollup_pss(Some(pid), &mut rss_bytes);

    ProcessMetrics {
        cpu_usage_pct: 0.0,
        vm_size_bytes,
        rss_bytes,
        pss_bytes,
    }
}

fn read_proc_status_memory(pid: Option<u32>) -> (u64, u64) {
    let mut path_buf = [0u8; 64];
    let path_str = match pid {
        Some(p) => {
            use std::io::Write;
            let mut cursor = std::io::Cursor::new(&mut path_buf[..]);
            let _ = write!(cursor, "/proc/{p}/status");
            let pos = cursor.position() as usize;
            std::str::from_utf8(&path_buf[..pos]).unwrap_or("/proc/self/status")
        }
        None => "/proc/self/status",
    };

    let mut vm_size = 0u64;
    let mut rss = 0u64;
    let mut file_buf = [0u8; 2048];

    if let Some(content) = read_file_to_buf(Path::new(path_str), &mut file_buf) {
        for line in content.lines() {
            if let Some(stripped) = line.strip_prefix("VmSize:") {
                vm_size = parse_kb_value(stripped) * 1024;
            } else if let Some(stripped) = line.strip_prefix("VmRSS:") {
                rss = parse_kb_value(stripped) * 1024;
            }
        }
    }

    (vm_size, rss)
}

fn read_proc_smaps_rollup_pss(pid: Option<u32>, rss_out: &mut u64) -> u64 {
    let mut path_buf = [0u8; 64];
    let path_str = match pid {
        Some(p) => {
            use std::io::Write;
            let mut cursor = std::io::Cursor::new(&mut path_buf[..]);
            let _ = write!(cursor, "/proc/{p}/smaps_rollup");
            let pos = cursor.position() as usize;
            std::str::from_utf8(&path_buf[..pos]).unwrap_or("/proc/self/smaps_rollup")
        }
        None => "/proc/self/smaps_rollup",
    };

    let mut file_buf = [0u8; 2048];
    if let Some(content) = read_file_to_buf(Path::new(path_str), &mut file_buf) {
        let mut pss = 0u64;
        for line in content.lines() {
            if let Some(stripped) = line.strip_prefix("Pss:") {
                pss = parse_kb_value(stripped) * 1024;
            } else if let Some(stripped) = line.strip_prefix("Rss:") {
                let r = parse_kb_value(stripped) * 1024;
                if r > 0 {
                    *rss_out = r;
                }
            }
        }
        if pss > 0 {
            return pss;
        }
    }

    // Fallback: If smaps_rollup is missing, use VmRSS as conservative approximation
    *rss_out
}

fn read_cpu_ticks(pid: Option<u32>) -> u64 {
    let mut path_buf = [0u8; 64];
    let path_str = match pid {
        Some(p) => {
            use std::io::Write;
            let mut cursor = std::io::Cursor::new(&mut path_buf[..]);
            let _ = write!(cursor, "/proc/{p}/stat");
            let pos = cursor.position() as usize;
            std::str::from_utf8(&path_buf[..pos]).unwrap_or("/proc/self/stat")
        }
        None => "/proc/self/stat",
    };

    let mut file_buf = [0u8; 2048];
    if let Some(content) = read_file_to_buf(Path::new(path_str), &mut file_buf) {
        if let Some(pos) = content.rfind(')') {
            let rest = &content[pos + 1..];
            let mut iter = rest.split_ascii_whitespace();
            // skip 0..10 fields after ')'
            let utime_str = iter.nth(10); // utime is 11th token (0-indexed 10)
            let stime_str = iter.next(); // stime is 12th token
            if let (Some(u), Some(s)) = (utime_str, stime_str) {
                let utime: u64 = u.parse().unwrap_or(0);
                let stime: u64 = s.parse().unwrap_or(0);
                return utime + stime;
            }
        }
    }

    0
}

fn parse_kb_value(line: &str) -> u64 {
    // Format: "   12345 kB"
    line.split_ascii_whitespace()
        .next()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
}
