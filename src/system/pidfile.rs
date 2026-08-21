use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
use std::os::unix::io::AsRawFd;

use crate::config::{LOCK_PATH, PID_PATH, RUN_DIR};
use crate::error::{CleanerError, Result};

pub struct PathPair {
    pub lock_path: &'static str,
    pub pid_path: &'static str,
}

// Mandatory primary paths with fallback for non-root local testing
pub const CANDIDATE_PATHS: &[PathPair] = &[
    PathPair {
        lock_path: LOCK_PATH,
        pid_path: PID_PATH,
    },
    PathPair {
        lock_path: "/tmp/cleaner_daemon.lock",
        pid_path: "/tmp/cleaner.pid",
    },
];

pub struct PidLock {
    #[cfg(unix)]
    lock_file: File,
    lock_path: PathBuf,
    pid_path: PathBuf,
}

impl PidLock {
    /// Attempts to acquire an exclusive lock on candidate lockfiles and write PID file.
    /// If another daemon instance holds the lock, returns `CleanerError::DaemonAlreadyRunning(pid)`.
    pub fn acquire() -> Result<Self> {
        #[cfg(unix)]
        {
            let mut last_err = None;

            // Ensure mandatory run directory exists
            let _ = fs::create_dir_all(RUN_DIR);

            for candidate in CANDIDATE_PATHS {
                if let Some(parent) = Path::new(candidate.lock_path).parent() {
                    if !parent.as_os_str().is_empty() && !parent.exists() {
                        let _ = fs::create_dir_all(parent);
                    }
                }

                match Self::try_lock_paths(candidate.lock_path, candidate.pid_path) {
                    Ok(lock) => return Ok(lock),
                    Err(CleanerError::DaemonAlreadyRunning(pid)) => {
                        return Err(CleanerError::DaemonAlreadyRunning(pid));
                    }
                    Err(e) => {
                        last_err = Some(e);
                    }
                }
            }

            Err(last_err.unwrap_or_else(|| {
                CleanerError::Other(format!(
                    "Failed to acquire PID lock on mandatory path {}",
                    LOCK_PATH
                ))
            }))
        }

        #[cfg(not(unix))]
        {
            Ok(Self {
                lock_path: PathBuf::from("cleaner_daemon.lock"),
                pid_path: PathBuf::from("cleaner.pid"),
            })
        }
    }

    #[cfg(unix)]
    fn try_lock_paths(lock_path_str: &str, pid_path_str: &str) -> Result<Self> {
        let lock_path = PathBuf::from(lock_path_str);
        let pid_path = PathBuf::from(pid_path_str);

        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o644)
            .open(&lock_path)?;

        let fd = lock_file.as_raw_fd();

        // Non-blocking exclusive advisory lock
        let res = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        if res != 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EWOULDBLOCK) || err.raw_os_error() == Some(libc::EAGAIN) {
                // Read PID from PID file
                let mut pid = 0;
                if let Ok(mut pf) = File::open(&pid_path) {
                    let mut content = String::new();
                    let _ = pf.read_to_string(&mut content);
                    pid = content.trim().parse::<u32>().unwrap_or(0);
                }
                return Err(CleanerError::DaemonAlreadyRunning(pid));
            }
            return Err(CleanerError::Io(err));
        }

        // Lock acquired! Write current PID to pid file
        let current_pid = std::process::id();
        let mut pid_file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o644)
            .open(&pid_path)?;

        writeln!(pid_file, "{}", current_pid)?;
        pid_file.flush()?;

        log::debug!(
            "Acquired exclusive lock at {} and wrote PID {} to {}",
            lock_path.display(),
            current_pid,
            pid_path.display()
        );

        Ok(Self {
            lock_file,
            lock_path,
            pid_path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.pid_path
    }

    #[allow(dead_code)]
    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }

    #[allow(dead_code)]
    pub fn pid_path(&self) -> &Path {
        &self.pid_path
    }
}

#[cfg(unix)]
impl Drop for PidLock {
    fn drop(&mut self) {
        let fd = self.lock_file.as_raw_fd();
        let _ = unsafe { libc::flock(fd, libc::LOCK_UN) };
        let _ = fs::remove_file(&self.pid_path);
        let _ = fs::remove_file(&self.lock_path);
        log::debug!("Released and cleaned PID lock files at {}", self.lock_path.display());
    }
}

/// Checks if a daemon is currently running by testing candidate lock and PID files.
pub fn get_running_pid() -> Option<u32> {
    #[cfg(unix)]
    {
        for candidate in CANDIDATE_PATHS {
            let lock_p = Path::new(candidate.lock_path);
            let pid_p = Path::new(candidate.pid_path);

            if lock_p.exists() {
                if let Ok(file) = OpenOptions::new().read(true).write(true).open(lock_p) {
                    let fd = file.as_raw_fd();
                    let res = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
                    if res != 0 {
                        // Lock is held -> Active running daemon!
                        if let Ok(mut pf) = File::open(pid_p) {
                            let mut content = String::new();
                            if pf.read_to_string(&mut content).is_ok() {
                                if let Ok(pid) = content.trim().parse::<u32>() {
                                    if is_process_alive(pid) {
                                        return Some(pid);
                                    }
                                }
                            }
                        }
                    } else {
                        // Lock wasn't held -> release test lock
                        let _ = unsafe { libc::flock(fd, libc::LOCK_UN) };
                        // Check if pid is alive anyway
                        if let Ok(mut pf) = File::open(pid_p) {
                            let mut content = String::new();
                            if pf.read_to_string(&mut content).is_ok() {
                                if let Ok(pid) = content.trim().parse::<u32>() {
                                    if is_process_alive(pid) {
                                        return Some(pid);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        None
    }

    #[cfg(not(unix))]
    {
        None
    }
}

/// Checks whether a process with the given PID is currently active.
pub fn is_process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        let res = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if res == 0 {
            true
        } else {
            let err = std::io::Error::last_os_error();
            // EPERM means process exists but belongs to another UID (e.g. root)
            err.raw_os_error() == Some(libc::EPERM)
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// Cleans up any stale, unheld PID and lock files.
pub fn clean_stale_pid_files() {
    #[cfg(unix)]
    {
        for candidate in CANDIDATE_PATHS {
            let lock_p = Path::new(candidate.lock_path);
            let pid_p = Path::new(candidate.pid_path);

            if lock_p.exists() {
                if let Ok(file) = OpenOptions::new().read(true).write(true).open(lock_p) {
                    let fd = file.as_raw_fd();
                    let res = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
                    if res == 0 {
                        // Acquired lock -> Safe to remove
                        let _ = unsafe { libc::flock(fd, libc::LOCK_UN) };
                        let _ = fs::remove_file(lock_p);
                        let _ = fs::remove_file(pid_p);
                    }
                }
            }
        }
    }
}
