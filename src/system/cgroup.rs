use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CgroupVersion {
    V1,
    V2,
    Hybrid,
    None,
}

impl std::fmt::Display for CgroupVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CgroupVersion::V1 => write!(f, "Cgroup v1 (Legacy Multi-Mount)"),
            CgroupVersion::V2 => write!(f, "Cgroup v2 (Unified Hierarchy)"),
            CgroupVersion::Hybrid => write!(f, "Hybrid (v1 + v2)"),
            CgroupVersion::None => write!(f, "None / Unsupported"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControllerMigration {
    pub controller: String,
    pub path: String,
    pub success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CgroupMigrationSummary {
    pub version: Option<CgroupVersion>,
    pub migrations: Vec<ControllerMigration>,
    pub fully_migrated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CgroupDiagnostics {
    pub detected_version: String,
    pub controllers_available: Vec<String>,
    pub current_process_cgroups: Vec<String>,
    pub supports_memory_reclaim: bool,
    pub active_reclaim_paths: Vec<String>,
}

/// Detects the active cgroup hierarchy mode on the Android device
#[must_use]
pub fn detect_cgroup_version() -> CgroupVersion {
    let v2_controllers = Path::new("/sys/fs/cgroup/cgroup.controllers");
    let v2_procs = Path::new("/sys/fs/cgroup/cgroup.procs");
    let has_v2 = v2_controllers.exists() || v2_procs.exists();

    let v1_paths = [
        "/dev/cpuset",
        "/dev/cpuctl",
        "/dev/stune",
        "/dev/blkio",
        "/dev/memcg",
    ];
    let has_v1 = v1_paths.iter().any(|p| Path::new(p).exists());

    match (has_v2, has_v1) {
        (true, true) => CgroupVersion::Hybrid,
        (true, false) => CgroupVersion::V2,
        (false, true) => CgroupVersion::V1,
        (false, false) => CgroupVersion::None,
    }
}

/// Checks if Cgroup v2 memory.reclaim interface is available in the kernel
#[allow(dead_code)]
#[must_use]
pub fn is_memory_reclaim_supported() -> bool {
    !discover_memory_reclaim_paths().is_empty()
}

/// Discovers all available memory.reclaim paths on the system
#[must_use]
pub fn discover_memory_reclaim_paths() -> Vec<PathBuf> {
    let candidate_roots = [
        "/sys/fs/cgroup/memory.reclaim",
        "/sys/fs/cgroup/background/memory.reclaim",
        "/sys/fs/cgroup/system-background/memory.reclaim",
        "/sys/fs/cgroup/apps/memory.reclaim",
    ];

    let mut found = Vec::new();
    for p in &candidate_roots {
        let path = Path::new(p);
        if path.exists() {
            found.push(path.to_path_buf());
        }
    }

    // Dynamic search for UID specific cgroups in /sys/fs/cgroup/
    if let Ok(entries) = fs::read_dir("/sys/fs/cgroup") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("uid_") {
                let uid_reclaim = entry.path().join("memory.reclaim");
                if uid_reclaim.exists() {
                    found.push(uid_reclaim);
                }
            }
        }
    }

    found
}

/// Reads the current process's cgroup memberships from `/proc/self/cgroup`
#[must_use]
pub fn read_current_process_cgroups() -> Vec<String> {
    #[cfg(unix)]
    {
        if let Ok(content) = fs::read_to_string("/proc/self/cgroup") {
            return content
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| line.trim().to_string())
                .collect();
        }
    }
    Vec::new()
}

/// Gathers comprehensive Cgroup diagnostics for debugging and status reporting
#[must_use]
pub fn get_cgroup_diagnostics() -> CgroupDiagnostics {
    let version = detect_cgroup_version();
    let reclaim_paths: Vec<String> = discover_memory_reclaim_paths()
        .into_iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();

    let mut controllers = Vec::new();

    // Read Cgroup v2 enabled controllers
    if let Ok(content) = fs::read_to_string("/sys/fs/cgroup/cgroup.controllers") {
        for c in content.split_whitespace() {
            controllers.push(format!("v2:{c}"));
        }
    }

    // Check Cgroup v1 controllers
    let v1_checks = [
        ("cpuset", "/dev/cpuset"),
        ("cpuctl", "/dev/cpuctl"),
        ("stune", "/dev/stune"),
        ("blkio", "/dev/blkio"),
        ("memcg", "/dev/memcg"),
    ];
    for (name, path) in &v1_checks {
        if Path::new(path).exists() {
            controllers.push(format!("v1:{name}"));
        }
    }

    CgroupDiagnostics {
        detected_version: format!("{version}"),
        controllers_available: controllers,
        current_process_cgroups: read_current_process_cgroups(),
        supports_memory_reclaim: !reclaim_paths.is_empty(),
        active_reclaim_paths: reclaim_paths,
    }
}

/// Migrates the current process into background cgroups across both Cgroup v1 & v2.
/// Guarantees that in Cgroup v1, all available subsystems (cpuset, cpuctl, stune, blkio)
/// are targeted without early exit.
#[must_use]
pub fn migrate_to_background_cgroup() -> CgroupMigrationSummary {
    let mut summary = CgroupMigrationSummary::default();

    #[cfg(unix)]
    {
        let pid = unsafe { libc::getpid() };
        let pid_str = format!("{pid}\n");
        let version = detect_cgroup_version();
        summary.version = Some(version);


        match version {
            CgroupVersion::V2 => {
                let v2_targets = [
                    "/sys/fs/cgroup/background/cgroup.procs",
                    "/sys/fs/cgroup/system-background/cgroup.procs",
                    "/sys/fs/cgroup/restricted/cgroup.procs",
                    "/sys/fs/cgroup/cgroup.procs",
                ];

                for path in &v2_targets {
                    if Path::new(path).exists() && fs::write(path, &pid_str).is_ok() {
                        log::debug!("Migrated PID {} to Cgroup v2: {}", pid, path);
                        summary.migrations.push(ControllerMigration {
                            controller: "unified_v2".to_string(),
                            path: path.to_string(),
                            success: true,
                        });
                        summary.fully_migrated = true;

                        // Apply low resource weights if controller files exist in target group
                        if let Some(parent) = Path::new(path).parent() {
                            let cpu_weight = parent.join("cpu.weight");
                            if cpu_weight.exists() {
                                let _ = fs::write(cpu_weight, "1\n");
                            }
                            let io_weight = parent.join("io.weight");
                            if io_weight.exists() {
                                let _ = fs::write(io_weight, "10\n");
                            }
                        }
                        break;
                    }
                }
            }

            CgroupVersion::V1 | CgroupVersion::Hybrid => {
                // In Cgroup v1 or Hybrid, iterate through EACH controller category independently
                let controller_groups = [
                    (
                        "cpuset",
                        &[
                            "/dev/cpuset/background/tasks",
                            "/dev/cpuset/system-background/tasks",
                            "/dev/cpuset/restricted/tasks",
                            "/dev/cpuset/tasks",
                        ][..],
                    ),
                    (
                        "cpuctl",
                        &[
                            "/dev/cpuctl/background/tasks",
                            "/dev/cpuctl/system-background/tasks",
                            "/dev/cpuctl/tasks",
                        ][..],
                    ),
                    (
                        "stune",
                        &["/dev/stune/background/tasks", "/dev/stune/tasks"][..],
                    ),
                    (
                        "blkio",
                        &[
                            "/dev/blkio/background/tasks",
                            "/dev/blkio/system-background/tasks",
                            "/dev/blkio/tasks",
                        ][..],
                    ),
                    ("memcg", &["/dev/memcg/apps/tasks", "/dev/memcg/tasks"][..]),
                ];

                let mut migrated_count = 0;
                for (name, candidates) in &controller_groups {
                    let mut matched = false;
                    for path in *candidates {
                        if Path::new(path).exists() && fs::write(path, &pid_str).is_ok() {
                            log::debug!("Migrated PID {} to Cgroup v1 {}: {}", pid, name, path);
                            summary.migrations.push(ControllerMigration {
                                controller: name.to_string(),
                                path: path.to_string(),
                                success: true,
                            });
                            matched = true;
                            migrated_count += 1;
                            break;
                        }
                    }
                    if !matched {
                        summary.migrations.push(ControllerMigration {
                            controller: name.to_string(),
                            path: "none_found".to_string(),
                            success: false,
                        });
                    }
                }

                // If hybrid, also attempt writing to v2 unified if available
                if version == CgroupVersion::Hybrid {
                    let v2_hybrid_paths = [
                        "/sys/fs/cgroup/background/cgroup.procs",
                        "/sys/fs/cgroup/system-background/cgroup.procs",
                        "/sys/fs/cgroup/cgroup.procs",
                    ];
                    for path in &v2_hybrid_paths {
                        if Path::new(path).exists() && fs::write(path, &pid_str).is_ok() {
                            log::debug!("Migrated PID {} to Hybrid v2 procs: {}", pid, path);
                            summary.migrations.push(ControllerMigration {
                                controller: "hybrid_v2".to_string(),
                                path: path.to_string(),
                                success: true,
                            });
                            break;
                        }
                    }
                }

                summary.fully_migrated = migrated_count > 0;
            }

            CgroupVersion::None => {
                log::debug!("No cgroups detected on device, staying in root namespace");
            }
        }
    }

    summary
}
