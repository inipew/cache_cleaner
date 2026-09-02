use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::os::unix::io::AsRawFd;

static LAST_TRIM_UNIX_SECS: AtomicU64 = AtomicU64::new(0);
static BYTES_DELETED_SINCE_LAST_TRIM: AtomicU64 = AtomicU64::new(0);
static STATE_INITIALIZED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Serialize, Deserialize, Default)]
struct FstrimPersistedState {
    last_trim_unix_secs: u64,
    bytes_deleted_since_trim: u64,
}

fn get_fstrim_state_path() -> PathBuf {
    let primary = Path::new("/data/adb/cleaner/fstrim_state.json");
    if primary.parent().is_some_and(|p| p.exists()) {
        primary.to_path_buf()
    } else {
        PathBuf::from("fstrim_state.json")
    }
}

fn ensure_initialized() {
    if !STATE_INITIALIZED.swap(true, Ordering::SeqCst) {
        let path = get_fstrim_state_path();
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(state) = serde_json::from_str::<FstrimPersistedState>(&content) {
                LAST_TRIM_UNIX_SECS.store(state.last_trim_unix_secs, Ordering::SeqCst);
                BYTES_DELETED_SINCE_LAST_TRIM.store(state.bytes_deleted_since_trim, Ordering::SeqCst);
            }
        }
    }
}

fn persist_state() {
    let state = FstrimPersistedState {
        last_trim_unix_secs: LAST_TRIM_UNIX_SECS.load(Ordering::SeqCst),
        bytes_deleted_since_trim: BYTES_DELETED_SINCE_LAST_TRIM.load(Ordering::SeqCst),
    };
    let path = get_fstrim_state_path();
    if let Ok(json_str) = serde_json::to_string_pretty(&state) {
        let tmp = path.with_extension("tmp");
        if fs::write(&tmp, json_str.as_bytes()).is_ok() {
            let _ = fs::rename(&tmp, &path);
        }
    }
}

pub fn record_freed_bytes_for_trim(freed_bytes: u64) {
    ensure_initialized();
    BYTES_DELETED_SINCE_LAST_TRIM.fetch_add(freed_bytes, Ordering::SeqCst);
    persist_state();
}

pub fn should_run_fstrim(manual_request: bool) -> bool {
    ensure_initialized();
    if manual_request {
        return true; // Manual request bypasses cooldown
    }

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let last = LAST_TRIM_UNIX_SECS.load(Ordering::SeqCst);
    let delta_bytes = BYTES_DELETED_SINCE_LAST_TRIM.load(Ordering::SeqCst);

    // Cooldown: at least 24h (86,400s) OR accumulated freed space >= 500 MB
    let time_eligible = last == 0 || (now_secs.saturating_sub(last) >= 86_400);
    let delta_eligible = delta_bytes >= 500_000_000;

    time_eligible || delta_eligible
}

pub fn mark_fstrim_completed() {
    ensure_initialized();
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    LAST_TRIM_UNIX_SECS.store(now_secs, Ordering::SeqCst);
    BYTES_DELETED_SINCE_LAST_TRIM.store(0, Ordering::SeqCst);
    persist_state();
}

#[cfg(unix)]
#[repr(C)]
struct FstrimRange {
    start: u64,
    len: u64,
    minlen: u64,
}

pub struct StorageOptimizer;

impl StorageOptimizer {
    /// Executes FITRIM strictly on configured allowlist mount points (or discovered candidate mounts if none specified)
    pub fn trim_mounts(configured_mounts: &[String]) -> bool {
        let mut overall_success = false;
        let target_mounts = if !configured_mounts.is_empty() {
            configured_mounts.to_vec()
        } else {
            Self::discover_trimmable_mounts()
        };

        for mount in &target_mounts {
            if !Path::new(mount).exists() {
                continue;
            }

            if Self::trim_single_mount(mount) {
                overall_success = true;
            }
        }

        overall_success
    }

    /// Discovers mounted read-write block filesystems suitable for FITRIM
    pub fn discover_trimmable_mounts() -> Vec<String> {
        let mut mounts = Vec::new();

        if let Ok(content) = fs::read_to_string("/proc/mounts") {
            for line in content.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 4 {
                    let dev = parts[0];
                    let mount_point = parts[1];
                    let fs_type = parts[2];
                    let options = parts[3];

                    // Check if it is a block device and read-write
                    let is_rw = options.split(',').any(|o| o == "rw");
                    // Strict whitelist: Only f2fs and ext4 natively support FITRIM safely on Android
                    let is_block_fs = matches!(fs_type, "f2fs" | "ext4");
                    let is_block_dev =
                        dev.starts_with("/dev/block/") || dev.starts_with("/dev/root");

                    if is_rw && is_block_fs && is_block_dev {
                        // Avoid virtual/runtime directories
                        if !mount_point.starts_with("/apex")
                            && !mount_point.starts_with("/mnt/runtime")
                            && !mount_point.starts_with("/mnt/pass_through")
                        {
                            mounts.push(mount_point.to_string());
                        }
                    }
                }
            }
        }

        mounts
    }

    fn trim_single_mount(mount_path: &str) -> bool {
        #[cfg(unix)]
        {
            if let Ok(file) = File::open(mount_path) {
                let fd = file.as_raw_fd();
                let mut range = FstrimRange {
                    start: 0,
                    len: u64::MAX,
                    minlen: 512 * 1024, // 512 KiB minimum extent size
                };

                #[cfg(target_os = "android")]
                const FITRIM_IOCTL: libc::c_int = 0xc018_5879_u32 as libc::c_int;
                #[cfg(not(target_os = "android"))]
                const FITRIM_IOCTL: libc::c_ulong = 0xc018_5879;

                let res = unsafe {
                    libc::ioctl(
                        fd,
                        FITRIM_IOCTL as _,
                        std::ptr::addr_of_mut!(range).cast::<libc::c_void>(),
                    )
                };

                if res == 0 {
                    log::info!("FITRIM trimmed {} bytes on {mount_path}", range.len);
                    return true;
                }
                log::debug!(
                    "ioctl(FITRIM) on {mount_path} failed: {}",
                    std::io::Error::last_os_error()
                );
            }
        }

        // Fallback: command execution
        let fallback_cmds = [
            ("fstrim", vec!["-v", mount_path]),
            ("/system/bin/fstrim", vec!["-v", mount_path]),
            ("sm", vec!["fstrim"]),
        ];

        for (cmd, args) in fallback_cmds {
            if let Ok(output) = Command::new(cmd).args(args).output() {
                if output.status.success() {
                    log::info!("fstrim succeeded on {} via {}", mount_path, cmd);
                    return true;
                }
            }
        }

        false
    }
}
