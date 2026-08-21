use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

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
    let path_str = match pid {
        Some(p) => format!("/proc/{p}/status"),
        None => "/proc/self/status".to_string(),
    };

    let mut vm_size = 0u64;
    let mut rss = 0u64;

    if let Ok(content) = fs::read_to_string(path_str) {
        for line in content.lines() {
            if line.starts_with("VmSize:") {
                vm_size = parse_kb_value(line) * 1024;
            } else if line.starts_with("VmRSS:") {
                rss = parse_kb_value(line) * 1024;
            }
        }
    }

    (vm_size, rss)
}

fn read_proc_smaps_rollup_pss(pid: Option<u32>, rss_out: &mut u64) -> u64 {
    let path_str = match pid {
        Some(p) => format!("/proc/{p}/smaps_rollup"),
        None => "/proc/self/smaps_rollup".to_string(),
    };

    let path = Path::new(&path_str);
    if path.exists() {
        if let Ok(content) = fs::read_to_string(path) {
            let mut pss = 0u64;
            for line in content.lines() {
                if line.starts_with("Pss:") {
                    pss = parse_kb_value(line) * 1024;
                } else if line.starts_with("Rss:") {
                    let r = parse_kb_value(line) * 1024;
                    if r > 0 {
                        *rss_out = r;
                    }
                }
            }
            if pss > 0 {
                return pss;
            }
        }
    }

    // Fallback: If smaps_rollup is missing, use VmRSS as conservative approximation
    *rss_out
}

fn read_cpu_ticks(pid: Option<u32>) -> u64 {
    let path_str = match pid {
        Some(p) => format!("/proc/{p}/stat"),
        None => "/proc/self/stat".to_string(),
    };

    if let Ok(content) = fs::read_to_string(path_str) {
        if let Some(pos) = content.rfind(')') {
            let rest = &content[pos + 1..];
            let fields: Vec<&str> = rest.split_whitespace().collect();
            // fields[0] is state (field 3 of /proc/<pid>/stat)
            // fields[11] is utime (field 14 of /proc/<pid>/stat)
            // fields[12] is stime (field 15 of /proc/<pid>/stat)
            if fields.len() > 12 {
                let utime: u64 = fields[11].parse().unwrap_or(0);
                let stime: u64 = fields[12].parse().unwrap_or(0);
                return utime + stime;
            }
        }
    }

    0
}

fn parse_kb_value(line: &str) -> u64 {
    // Format: "Key:\t   12345 kB"
    line.split_whitespace()
        .nth(1)
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
}
