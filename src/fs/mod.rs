use std::ffi::CString;
use std::mem::MaybeUninit;
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use rustix::fd::{AsFd, BorrowedFd, OwnedFd};
use rustix::fs::{
    fstat, openat, statat, unlinkat, AtFlags, FileType, Mode, OFlags, RawDir,
};

use crate::domain::types::{DeviceNumber, FileIdentity, InodeNumber};
use crate::error::{CleanerError, Result};
use crate::resource::FdPermit;

#[derive(Debug, Clone)]
pub struct SafeDirEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub identity: FileIdentity,
    pub size_bytes: u64,
    pub mtime_secs: u64,
}

#[repr(C)]
struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}

const RESOLVE_NO_XDEV: u64 = 0x01;
const RESOLVE_NO_MAGICLINKS: u64 = 0x02;
const RESOLVE_NO_SYMLINKS: u64 = 0x04;
const RESOLVE_BENEATH: u64 = 0x08;

static OPENAT2_SUPPORTED: AtomicBool = AtomicBool::new(true);

/// RAII wrapper for a safe directory file descriptor preventing path-based symlink attacks.
#[derive(Debug)]
pub struct SafeDirHandle {
    fd: OwnedFd,
    dev: DeviceNumber,
    ino: InodeNumber,
    _permit: Option<FdPermit>,
}

impl SafeDirHandle {
    /// Opens a trusted root directory. Enforces `O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC`.
    pub fn open_root(path: &Path) -> Result<Self> {
        Self::open_root_with_permit(path, None)
    }

    /// Opens a trusted root directory holding an optional FD resource permit.
    pub fn open_root_with_permit(path: &Path, permit: Option<FdPermit>) -> Result<Self> {
        let oflags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
        let fd = rustix::fs::open(path, oflags, Mode::empty())
            .map_err(|e| CleanerError::Io(std::io::Error::from_raw_os_error(e.raw_os_error())))?;

        let st = fstat(&fd)
            .map_err(|e| CleanerError::Io(std::io::Error::from_raw_os_error(e.raw_os_error())))?;

        Ok(Self {
            fd,
            dev: DeviceNumber(st.st_dev),
            ino: InodeNumber(st.st_ino),
            _permit: permit,
        })
    }

    pub fn device(&self) -> DeviceNumber {
        self.dev
    }

    pub fn inode(&self) -> InodeNumber {
        self.ino
    }

    pub fn identity(&self) -> FileIdentity {
        FileIdentity {
            dev: self.dev,
            ino: self.ino,
        }
    }

    pub fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }

    pub fn raw_fd(&self) -> std::os::unix::io::RawFd {
        self.fd.as_raw_fd()
    }

    /// Query metadata of a child without following symlinks.
    pub fn stat_child(&self, child_name: &str) -> Result<FileIdentity> {
        let st = statat(self.as_fd(), child_name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|e| CleanerError::Io(std::io::Error::from_raw_os_error(e.raw_os_error())))?;

        Ok(FileIdentity::new(st.st_dev, st.st_ino))
    }

    /// Safely open a sub-directory relative to this directory.
    /// Uses openat2 probe with RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS if supported, falling back to openat + dev check.
    pub fn open_child_dir(&self, child_name: &str) -> Result<Self> {
        self.open_child_dir_with_permit(child_name, None)
    }

    /// Safely open a sub-directory relative to this directory with an FD permit.
    pub fn open_child_dir_with_permit(&self, child_name: &str, permit: Option<FdPermit>) -> Result<Self> {
        let child_fd = self.try_openat2_child(child_name)
            .or_else(|| self.openat_fallback(child_name))
            .ok_or_else(|| CleanerError::SafetyViolation(format!("Failed to securely open child directory {}", child_name)))?;

        let st = fstat(&child_fd)
            .map_err(|e| CleanerError::Io(std::io::Error::from_raw_os_error(e.raw_os_error())))?;

        let child_dev = DeviceNumber(st.st_dev);
        if child_dev != self.dev {
            return Err(CleanerError::SafetyViolation(format!(
                "Anti-mount crossing violation: child directory {} has device {} != parent device {}",
                child_name, child_dev, self.dev
            )));
        }

        Ok(Self {
            fd: child_fd,
            dev: child_dev,
            ino: InodeNumber(st.st_ino),
            _permit: permit,
        })
    }

    fn try_openat2_child(&self, child_name: &str) -> Option<OwnedFd> {
        if !OPENAT2_SUPPORTED.load(Ordering::Relaxed) {
            return None;
        }

        #[cfg(target_os = "linux")]
        {
            let c_name = CString::new(child_name).ok()?;
            let how = OpenHow {
                flags: (libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW) as u64,
                mode: 0,
                resolve: RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS | RESOLVE_NO_XDEV,
            };

            let res = unsafe {
                libc::syscall(
                    libc::SYS_openat2,
                    self.raw_fd(),
                    c_name.as_ptr(),
                    &how as *const OpenHow,
                    std::mem::size_of::<OpenHow>(),
                )
            };

            if res >= 0 {
                use std::os::unix::io::FromRawFd;
                return Some(unsafe { OwnedFd::from_raw_fd(res as i32) });
            }

            let err = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            if err == libc::ENOSYS {
                OPENAT2_SUPPORTED.store(false, Ordering::Relaxed);
            }
        }
        None
    }

    fn openat_fallback(&self, child_name: &str) -> Option<OwnedFd> {
        let oflags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
        openat(self.as_fd(), child_name, oflags, Mode::empty()).ok()
    }

    /// Read directory entries directly from the open file descriptor via `RawDir`.
    pub fn read_entries_fd(&self) -> Result<Vec<SafeDirEntry>> {
        let mut buf = [MaybeUninit::uninit(); 8192];
        let mut raw_dir = RawDir::new(self.as_fd(), &mut buf);
        let mut entries = Vec::new();

        while let Some(entry_res) = raw_dir.next() {
            let entry = entry_res
                .map_err(|e| CleanerError::Io(std::io::Error::from_raw_os_error(e.raw_os_error())))?;

            let file_name_bytes = entry.file_name().to_bytes();
            if file_name_bytes == b"." || file_name_bytes == b".." {
                continue;
            }

            let name = match std::str::from_utf8(file_name_bytes) {
                Ok(s) => s.to_string(),
                Err(_) => continue,
            };

            let ft = entry.file_type();
            let is_symlink = ft == FileType::Symlink;
            let is_dir = ft == FileType::Directory;

            // Stat child directly via statat on descriptor
            if let Ok(st) = statat(self.as_fd(), &name, AtFlags::SYMLINK_NOFOLLOW) {
                entries.push(SafeDirEntry {
                    name,
                    is_dir,
                    is_symlink,
                    identity: FileIdentity::new(st.st_dev, st.st_ino),
                    size_bytes: st.st_size.max(0) as u64,
                    #[allow(clippy::unnecessary_cast)]
                    mtime_secs: st.st_mtime as u64,
                });
            }
        }

        Ok(entries)
    }

    /// Atomic deletion of a file relative to this directory descriptor with identity revalidation.
    pub fn unlink_child_file(&self, file_name: &str, expected_identity: &FileIdentity) -> Result<()> {
        let current_identity = self.stat_child(file_name)?;
        if current_identity != *expected_identity {
            return Err(CleanerError::SafetyViolation(format!(
                "TOCTOU race detected: file {} identity {} does not match expected {}",
                file_name, current_identity, expected_identity
            )));
        }

        unlinkat(self.as_fd(), file_name, AtFlags::empty())
            .map_err(|e| CleanerError::Io(std::io::Error::from_raw_os_error(e.raw_os_error())))
    }

    /// Atomic removal of an empty sub-directory relative to this descriptor with identity revalidation.
    pub fn rmdir_child_dir(&self, dir_name: &str, expected_identity: &FileIdentity) -> Result<()> {
        let current_identity = self.stat_child(dir_name)?;
        if current_identity != *expected_identity {
            return Err(CleanerError::SafetyViolation(format!(
                "TOCTOU race detected: dir {} identity {} does not match expected {}",
                dir_name, current_identity, expected_identity
            )));
        }

        unlinkat(self.as_fd(), dir_name, AtFlags::REMOVEDIR)
            .map_err(|e| CleanerError::Io(std::io::Error::from_raw_os_error(e.raw_os_error())))
    }
}
