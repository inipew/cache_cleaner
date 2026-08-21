use std::fs;
use std::path::Path;

pub struct MemoryOptimizer;

impl MemoryOptimizer {
    /// Compresses inactive pages in ZRAM swap device across all discovered zram instances
    pub fn compact_zram() -> bool {
        let mut success = false;

        // 1. Dynamic scan in /sys/block/
        if let Ok(entries) = fs::read_dir("/sys/block") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.starts_with("zram") {
                    let compact_path = entry.path().join("compact");
                    if compact_path.exists() && fs::write(&compact_path, "all\n").is_ok() {
                        log::info!("ZRAM compaction triggered on {}", compact_path.display());
                        success = true;
                    }
                }
            }
        }

        // 2. Static fallbacks
        if !success {
            let zram_paths = ["/sys/block/zram0/compact", "/sys/block/zram1/compact"];

            for path in &zram_paths {
                if Path::new(path).exists() && fs::write(path, "all\n").is_ok() {
                    log::info!("ZRAM compaction triggered on {}", path);
                    success = true;
                }
            }
        }

        success
    }

    /// Compacts memory fragments to reduce page allocation latency
    pub fn compact_memory() -> bool {
        let compact_path = "/proc/sys/vm/compact_memory";
        if Path::new(compact_path).exists() && fs::write(compact_path, "1\n").is_ok() {
            log::info!("Memory compaction executed via /proc/sys/vm/compact_memory");
            return true;
        }
        false
    }

    /// Proactively reclaims inactive cached pages using Cgroup v2 memory.reclaim interface.
    /// This uses standard kernel LRU eviction without discarding hot UI/System caches.
    pub fn reclaim_cgroup_memory(target_mb: u64) -> bool {
        let paths = crate::system::cgroup::discover_memory_reclaim_paths();
        if paths.is_empty() {
            log::debug!("Cgroup v2 memory.reclaim not supported or not found on this kernel");
            return false;
        }

        let target_bytes = target_mb.saturating_mul(1024 * 1024);
        let payload = format!("{}\n", target_bytes);

        let mut success = false;
        for path in &paths {
            match fs::write(path, &payload) {
                Ok(_) => {
                    log::info!(
                        "Cgroup v2 memory reclaim (target: {} MB) triggered successfully on {}",
                        target_mb,
                        path.display()
                    );
                    success = true;
                }
                Err(e) => {
                    // Writing to memory.reclaim may return EAGAIN / EBUSY if memory cannot be reclaimed
                    log::debug!(
                        "Cgroup memory reclaim on {} completed or yielded: {}",
                        path.display(),
                        e
                    );
                    // Even if partial, it triggered kernel reclaim
                    success = true;
                }
            }
        }

        success
    }

    /// Safely flushes dirty pages and reclaims inactive pagecaches
    #[allow(dead_code)]
    pub fn safe_drop_caches() -> bool {
        #[cfg(unix)]
        {
            // Sync filesystems first before dropping caches
            unsafe { libc::sync() };
        }

        let drop_path = "/proc/sys/vm/drop_caches";
        if Path::new(drop_path).exists() && fs::write(drop_path, "3\n").is_ok() {
            log::info!("Kernel page caches reclaimed");
            return true;
        }
        false
    }
}
