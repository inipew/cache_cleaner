#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::path::Path;

pub fn migrate_to_background_cgroup() {
    #[cfg(unix)]
    {
        let pid = unsafe { libc::getpid() };
        let pid_str = format!("{}\n", pid);

        let cgroup_tasks = [
            "/dev/cpuset/background/tasks",
            "/dev/cpuset/system-background/tasks",
            "/dev/cpuset/restricted/tasks",
            "/sys/fs/cgroup/background/cgroup.procs",
            "/sys/fs/cgroup/system-background/cgroup.procs",
            "/dev/stune/background/tasks",
            "/dev/cpuctl/background/tasks",
        ];

        let mut migrated = false;
        for path in &cgroup_tasks {
            if Path::new(path).exists() {
                if let Ok(_) = fs::write(path, &pid_str) {
                    log::debug!("Migrated PID {} to cgroup: {}", pid, path);
                    migrated = true;
                    break;
                }
            }
        }

        if !migrated {
            log::debug!("No specific background cgroup found, staying in default group");
        }
    }
}
