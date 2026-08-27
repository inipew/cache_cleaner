use std::fs;
#[cfg(unix)]
use std::fs::File;
use std::path::Path;
use std::process::Command;

#[cfg(unix)]
use std::os::unix::io::AsRawFd;

#[cfg(unix)]
#[repr(C)]
struct FstrimRange {
    start: u64,
    len: u64,
    minlen: u64,
}

pub struct StorageOptimizer;

impl StorageOptimizer {
    /// Executes FITRIM on specified mount points (e.g. /data, /cache) and discovered mounts
    pub fn trim_mounts(configured_mounts: &[String]) -> bool {
        let mut overall_success = false;
        let mut target_mounts = configured_mounts.to_vec();

        // Dynamically discover candidate partitions from /proc/mounts
        for discovered in Self::discover_trimmable_mounts() {
            if !target_mounts.contains(&discovered) {
                target_mounts.push(discovered);
            }
        }

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
