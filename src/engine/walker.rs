#[cfg(unix)]
use rustix::fs::{fstat, openat, statat, unlinkat, AtFlags, FileType, Mode, OFlags, RawDir, CWD};
#[cfg(not(unix))]
use std::fs;
#[cfg(unix)]
use std::mem::MaybeUninit;
use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::engine::cancellation::CancellationToken;
use crate::engine::rules::{Decision, RuleEngine};

#[derive(Debug, Default, Clone)]
pub struct WalkStats {
    pub files_deleted: usize,
    pub bytes_freed: u64,
    pub frozen_apps_affected: usize,
    pub active_apps_affected: usize,
    pub skipped_files: usize,
    pub errors_count: usize,
}

#[inline]
#[allow(clippy::useless_conversion, clippy::unnecessary_cast)]
fn st_mtime_to_duration<T: TryInto<u64>>(st_mtime: T) -> Duration {
    Duration::from_secs(st_mtime.try_into().unwrap_or(0))
}

#[cfg(unix)]
#[inline]
fn safe_unlink_entry<Fd: rustix::fd::AsFd>(
    dir_fd: &Fd,
    name: &str,
    expected_dev: u64,
    expected_ino: u64,
    expected_ft: FileType,
    flags: AtFlags,
) -> bool {
    // Inode Identity Revalidation immediately prior to unlinkat:
    // Guarantees device ID, inode number, and file mode have not been swapped by an adversary/race
    if let Ok(st_latest) = statat(dir_fd, name, AtFlags::SYMLINK_NOFOLLOW) {
        if (st_latest.st_dev as u64) != expected_dev {
            log::warn!("Aborting unlink: st_dev mismatch on {}", name);
            return false;
        }
        if st_latest.st_ino != expected_ino {
            log::warn!("Aborting unlink: st_ino changed between stat and unlink on {}", name);
            return false;
        }
        let current_ft = FileType::from_raw_mode(st_latest.st_mode);
        if current_ft != expected_ft {
            log::warn!("Aborting unlink: FileType mutated on {}", name);
            return false;
        }
        unlinkat(dir_fd, name, flags).is_ok()
    } else {
        false
    }
}

use crate::util::TokenBucketRateLimiter;

pub struct DirectoryWalker<'a> {
    rule_engine: &'a RuleEngine,
    cancel_token: &'a CancellationToken,
    min_age: Duration,
    dry_run: bool,
    frozen_uids: Option<&'a std::collections::HashSet<u32>>,
    rate_limiter: Option<&'a TokenBucketRateLimiter>,
}

impl<'a> DirectoryWalker<'a> {
    pub fn new(
        rule_engine: &'a RuleEngine,
        cancel_token: &'a CancellationToken,
        min_age_hours: u32,
        dry_run: bool,
    ) -> Self {
        Self {
            rule_engine,
            cancel_token,
            min_age: Duration::from_secs(u64::from(min_age_hours) * 3600),
            dry_run,
            frozen_uids: None,
            rate_limiter: None,
        }
    }

    pub fn with_frozen_uids(
        mut self,
        frozen_uids: Option<&'a std::collections::HashSet<u32>>,
    ) -> Self {
        self.frozen_uids = frozen_uids;
        self
    }

    pub fn with_rate_limiter(
        mut self,
        rate_limiter: Option<&'a TokenBucketRateLimiter>,
    ) -> Self {
        self.rate_limiter = rate_limiter;
        self
    }

    /// Recursively purge junk files inside a directory tree safely using zero-copy getdents64,
    /// fd-relative syscalls, and strict mount boundary enforcement.
    pub fn clean_directory(&self, root: &Path) -> WalkStats {
        let mut stats = WalkStats::default();

        #[cfg(unix)]
        {
            let dir_fd = match openat(
                CWD,
                root,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::empty(),
            ) {
                Ok(fd) => fd,
                Err(err) => {
                    log::debug!(
                        "Skipping root directory {} (open failed: {})",
                        root.display(),
                        err
                    );
                    stats.skipped_files += 1;
                    return stats;
                }
            };

            let root_stat = match fstat(&dir_fd) {
                Ok(st) => st,
                Err(_) => return stats,
            };

            let root_dev = root_stat.st_dev;
            drop(dir_fd);

            self.walk_internal(root, root_dev, &mut stats, 0);
        }

        #[cfg(not(unix))]
        {
            if !root.exists() {
                return stats;
            }
            self.walk_internal(root, 0, &mut stats, 0);
        }

        stats
    }

    /// Clean crash dump directory retaining the newest N files strictly using fd-relative syscalls
    pub fn clean_crash_dumps_directory(&self, dir: &Path, keep_count: usize) -> WalkStats {
        let mut stats = WalkStats::default();

        #[cfg(unix)]
        {
            let dir_fd = match openat(
                CWD,
                dir,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::empty(),
            ) {
                Ok(fd) => fd,
                Err(err) => {
                    log::debug!(
                        "Skipping crash dump dir {} (open failed: {})",
                        dir.display(),
                        err
                    );
                    stats.skipped_files += 1;
                    return stats;
                }
            };

            let mut crash_files: Vec<(String, u64, SystemTime, u64, u64)> = Vec::new();
            let mut buf = [MaybeUninit::uninit(); 8192];
            let mut raw_dir = RawDir::new(&dir_fd, &mut buf);

            while let Some(entry_res) = raw_dir.next() {
                if self.cancel_token.is_cancelled() {
                    break;
                }

                if let Ok(entry) = entry_res {
                    let name_bytes = entry.file_name().to_bytes();
                    if name_bytes == b"." || name_bytes == b".." {
                        continue;
                    }

                    if entry.file_type() == FileType::RegularFile {
                        if let Ok(name_str) = std::str::from_utf8(name_bytes) {
                            if let Ok(st) = statat(&dir_fd, name_str, AtFlags::SYMLINK_NOFOLLOW) {
                                let mtime =
                                    SystemTime::UNIX_EPOCH + st_mtime_to_duration(st.st_mtime);
                                crash_files.push((
                                    name_str.to_string(),
                                    st.st_size as u64,
                                    mtime,
                                    st.st_dev as u64,
                                    st.st_ino,
                                ));
                            }
                        }
                    }
                }
            }

            // Sort descending by modified time (newest first)
            crash_files.sort_by_key(|b| std::cmp::Reverse(b.2));

            // Skip newest `keep_count` files, delete older ones via fd-relative unlinkat with inode revalidation
            for (name, size, _, dev, ino) in crash_files.into_iter().skip(keep_count) {
                if self.cancel_token.is_cancelled() {
                    break;
                }

                if let Some(limiter) = self.rate_limiter {
                    if !limiter.acquire() && self.cancel_token.is_cancelled() {
                        break;
                    }
                }

                if !self.dry_run {
                    if safe_unlink_entry(&dir_fd, &name, dev, ino, FileType::RegularFile, AtFlags::empty()) {
                        stats.files_deleted += 1;
                        stats.bytes_freed += size;
                    } else {
                        stats.errors_count += 1;
                    }
                } else {
                    stats.files_deleted += 1;
                    stats.bytes_freed += size;
                }
            }
        }

        #[cfg(not(unix))]
        {
            if let Ok(entries) = fs::read_dir(dir) {
                let mut crash_files: Vec<(std::path::PathBuf, u64, SystemTime)> = Vec::new();
                for entry in entries.flatten() {
                    if self.cancel_token.is_cancelled() {
                        break;
                    }

                    let path = entry.path();
                    if let Ok(meta) = entry.metadata() {
                        if meta.is_file() {
                            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                            crash_files.push((path, meta.len(), mtime));
                        }
                    }
                }

                crash_files.sort_by_key(|b| std::cmp::Reverse(b.2));

                for (path, size, _) in crash_files.into_iter().skip(keep_count) {
                    if self.cancel_token.is_cancelled() {
                        break;
                    }

                    if !self.dry_run {
                        if fs::remove_file(&path).is_ok() {
                            stats.files_deleted += 1;
                            stats.bytes_freed += size;
                        } else {
                            stats.errors_count += 1;
                        }
                    } else {
                        stats.files_deleted += 1;
                        stats.bytes_freed += size;
                    }
                }
            }
        }

        stats
    }

    #[allow(unused_variables)]
    fn walk_internal(&self, dir: &Path, root_dev: u64, stats: &mut WalkStats, depth: usize) {
        if depth > 20 || self.cancel_token.is_cancelled() {
            return;
        }

        #[cfg(unix)]
        {
            let dir_fd = match openat(
                CWD,
                dir,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::empty(),
            ) {
                Ok(fd) => fd,
                Err(err) => {
                    log::debug!(
                        "Skipping directory {} (open failed: {})",
                        dir.display(),
                        err
                    );
                    stats.skipped_files += 1;
                    return;
                }
            };

            let mut buf = [MaybeUninit::uninit(); 8192];
            let mut raw_dir = RawDir::new(&dir_fd, &mut buf);
            let now = SystemTime::now();

            while let Some(entry_result) = raw_dir.next() {
                if self.cancel_token.is_cancelled() {
                    break;
                }

                if let Some(limiter) = self.rate_limiter {
                    if !limiter.acquire() && self.cancel_token.is_cancelled() {
                        break;
                    }
                }

                let entry = match entry_result {
                    Ok(e) => e,
                    Err(_) => {
                        stats.errors_count += 1;
                        continue;
                    }
                };

                let file_name_bytes = entry.file_name().to_bytes();
                if file_name_bytes == b"." || file_name_bytes == b".." {
                    continue;
                }

                let name_str = match std::str::from_utf8(file_name_bytes) {
                    Ok(s) => s,
                    Err(_) => {
                        stats.skipped_files += 1;
                        continue;
                    }
                };

                let path = dir.join(name_str);
                let file_type = entry.file_type();

                // Mount boundary & Symlink check
                let st = match statat(&dir_fd, name_str, AtFlags::SYMLINK_NOFOLLOW) {
                    Ok(st) => st,
                    Err(_) => {
                        stats.errors_count += 1;
                        continue;
                    }
                };

                // Never cross mount boundary (e.g. into mounted APEX / FUSE / other partitions)
                if (st.st_dev as u64) != root_dev {
                    log::debug!(
                        "Skipping cross-mount boundary at {} (dev {} != root_dev {})",
                        path.display(),
                        st.st_dev,
                        root_dev
                    );
                    stats.skipped_files += 1;
                    continue;
                }

                let decision = self.rule_engine.evaluate_path(&path);

                match decision {
                    Decision::Skip { .. } => {
                        if file_type == FileType::Directory {
                            if depth == 0 {
                                let uid = st.st_uid;
                                if uid >= 10000 {
                                    if let Some(frozen_set) = self.frozen_uids {
                                        if frozen_set.contains(&uid) {
                                            stats.frozen_apps_affected += 1;
                                        } else {
                                            stats.active_apps_affected += 1;
                                        }
                                    }
                                }
                            }
                            self.walk_internal(&path, root_dev, stats, depth + 1);
                        }
                    }
                    Decision::Delete { .. } => {
                        if file_type == FileType::Symlink {
                            if !self.dry_run {
                                if safe_unlink_entry(&dir_fd, name_str, root_dev, st.st_ino, FileType::Symlink, AtFlags::empty()) {
                                    stats.files_deleted += 1;
                                } else {
                                    stats.errors_count += 1;
                                }
                            } else {
                                stats.files_deleted += 1;
                            }
                        } else if file_type == FileType::RegularFile {
                            let file_size = st.st_size as u64;

                            // Strict timestamp check: handle clock skew / future timestamps safely
                            if self.min_age.as_secs() > 0 {
                                let mtime =
                                    SystemTime::UNIX_EPOCH + st_mtime_to_duration(st.st_mtime);
                                match now.duration_since(mtime) {
                                    Ok(elapsed) if elapsed < self.min_age => {
                                        stats.skipped_files += 1;
                                        continue;
                                    }
                                    Err(_) => {
                                        // Future timestamp -> clock anomaly -> skip safely!
                                        stats.skipped_files += 1;
                                        continue;
                                    }
                                    _ => {}
                                }
                            }

                            if !self.dry_run {
                                if safe_unlink_entry(&dir_fd, name_str, root_dev, st.st_ino, FileType::RegularFile, AtFlags::empty()) {
                                    stats.files_deleted += 1;
                                    stats.bytes_freed += file_size;
                                } else {
                                    stats.errors_count += 1;
                                }
                            } else {
                                stats.files_deleted += 1;
                                stats.bytes_freed += file_size;
                            }
                        } else if file_type == FileType::Directory {
                            self.purge_folder_contents(&path, root_dev, stats, depth + 1);
                        }
                    }
                }
            }
        }

        #[cfg(not(unix))]
        {
            let entries = match fs::read_dir(dir) {
                Ok(e) => e,
                Err(_) => return,
            };

            let now = SystemTime::now();

            for entry in entries.flatten() {
                if self.cancel_token.is_cancelled() {
                    break;
                }

                let path = entry.path();
                let file_type = match entry.file_type() {
                    Ok(ft) => ft,
                    Err(_) => continue,
                };

                let decision = self.rule_engine.evaluate_path(&path);

                match decision {
                    Decision::Skip { .. } => {
                        if file_type.is_dir() && !file_type.is_symlink() {
                            self.walk_internal(&path, root_dev, stats, depth + 1);
                        }
                    }
                    Decision::Delete { .. } => {
                        if file_type.is_symlink() {
                            if !self.dry_run {
                                if fs::remove_file(&path).is_ok() {
                                    stats.files_deleted += 1;
                                } else {
                                    stats.errors_count += 1;
                                }
                            } else {
                                stats.files_deleted += 1;
                            }
                        } else if file_type.is_file() {
                            if let Ok(metadata) = entry.metadata() {
                                let file_size = metadata.len();
                                if self.min_age.as_secs() > 0 {
                                    if let Ok(modified) = metadata.modified() {
                                        match now.duration_since(modified) {
                                            Ok(elapsed) if elapsed < self.min_age => {
                                                stats.skipped_files += 1;
                                                continue;
                                            }
                                            Err(_) => {
                                                stats.skipped_files += 1;
                                                continue;
                                            }
                                            _ => {}
                                        }
                                    }
                                }

                                if !self.dry_run {
                                    if fs::remove_file(&path).is_ok() {
                                        stats.files_deleted += 1;
                                        stats.bytes_freed += file_size;
                                    } else {
                                        stats.errors_count += 1;
                                    }
                                } else {
                                    stats.files_deleted += 1;
                                    stats.bytes_freed += file_size;
                                }
                            }
                        } else if file_type.is_dir() {
                            self.purge_folder_contents(&path, root_dev, stats, depth + 1);
                        }
                    }
                }
            }
        }
    }

    #[allow(unused_variables)]
    fn purge_folder_contents(
        &self,
        folder: &Path,
        root_dev: u64,
        stats: &mut WalkStats,
        depth: usize,
    ) {
        if depth > 20 || self.cancel_token.is_cancelled() {
            return;
        }

        #[cfg(unix)]
        {
            let dir_fd = match openat(
                CWD,
                folder,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::empty(),
            ) {
                Ok(fd) => fd,
                Err(_) => return,
            };

            let mut buf = [MaybeUninit::uninit(); 8192];
            let mut raw_dir = RawDir::new(&dir_fd, &mut buf);
            let now = SystemTime::now();

            while let Some(entry_result) = raw_dir.next() {
                if self.cancel_token.is_cancelled() {
                    break;
                }

                if let Some(limiter) = self.rate_limiter {
                    if !limiter.acquire() && self.cancel_token.is_cancelled() {
                        break;
                    }
                }

                let entry = match entry_result {
                    Ok(e) => e,
                    Err(_) => continue,
                };

                let file_name_bytes = entry.file_name().to_bytes();
                if file_name_bytes == b"." || file_name_bytes == b".." {
                    continue;
                }

                let name_str = match std::str::from_utf8(file_name_bytes) {
                    Ok(s) => s,
                    Err(_) => continue,
                };

                let p = folder.join(name_str);
                let file_type = entry.file_type();

                let st = match statat(&dir_fd, name_str, AtFlags::SYMLINK_NOFOLLOW) {
                    Ok(st) => st,
                    Err(_) => continue,
                };

                if (st.st_dev as u64) != root_dev {
                    stats.skipped_files += 1;
                    continue;
                }

                let decision = self.rule_engine.evaluate_path(&p);

                if let Decision::Skip { .. } = decision {
                    if file_type == FileType::Directory {
                        self.walk_internal(&p, root_dev, stats, depth + 1);
                    }
                    continue;
                }

                if file_type == FileType::Symlink {
                    if !self.dry_run {
                        if safe_unlink_entry(&dir_fd, name_str, root_dev, st.st_ino, FileType::Symlink, AtFlags::empty()) {
                            stats.files_deleted += 1;
                        } else {
                            stats.errors_count += 1;
                        }
                    } else {
                        stats.files_deleted += 1;
                    }
                } else if file_type == FileType::Directory {
                    self.purge_folder_contents(&p, root_dev, stats, depth + 1);
                    if !self.dry_run {
                        let _ = safe_unlink_entry(&dir_fd, name_str, root_dev, st.st_ino, FileType::Directory, AtFlags::REMOVEDIR);
                    }
                } else if file_type == FileType::RegularFile {
                    if self.min_age.as_secs() > 0 {
                        let mtime = SystemTime::UNIX_EPOCH + st_mtime_to_duration(st.st_mtime);
                        match now.duration_since(mtime) {
                            Ok(elapsed) if elapsed < self.min_age => {
                                stats.skipped_files += 1;
                                continue;
                            }
                            Err(_) => {
                                stats.skipped_files += 1;
                                continue;
                            }
                            _ => {}
                        }
                    }

                    let size = st.st_size as u64;
                    if !self.dry_run {
                        if safe_unlink_entry(&dir_fd, name_str, root_dev, st.st_ino, FileType::RegularFile, AtFlags::empty()) {
                            stats.files_deleted += 1;
                            stats.bytes_freed += size;
                        } else {
                            stats.errors_count += 1;
                        }
                    } else {
                        stats.files_deleted += 1;
                        stats.bytes_freed += size;
                    }
                }
            }
        }

        #[cfg(not(unix))]
        {
            let entries = match fs::read_dir(folder) {
                Ok(e) => e,
                Err(_) => return,
            };

            let now = SystemTime::now();

            for entry in entries.flatten() {
                if self.cancel_token.is_cancelled() {
                    break;
                }

                let p = entry.path();
                let file_type = match entry.file_type() {
                    Ok(ft) => ft,
                    Err(_) => continue,
                };

                let decision = self.rule_engine.evaluate_path(&p);

                if let Decision::Skip { .. } = decision {
                    if file_type.is_dir() && !file_type.is_symlink() {
                        self.walk_internal(&p, root_dev, stats, depth + 1);
                    }
                    continue;
                }

                if file_type.is_symlink() {
                    if !self.dry_run {
                        if fs::remove_file(&p).is_ok() {
                            stats.files_deleted += 1;
                        } else {
                            stats.errors_count += 1;
                        }
                    } else {
                        stats.files_deleted += 1;
                    }
                } else if file_type.is_dir() {
                    self.purge_folder_contents(&p, root_dev, stats, depth + 1);
                    if !self.dry_run {
                        let _ = fs::remove_dir(&p);
                    }
                } else if file_type.is_file() {
                    if let Ok(metadata) = entry.metadata() {
                        if self.min_age.as_secs() > 0 {
                            if let Ok(modified) = metadata.modified() {
                                match now.duration_since(modified) {
                                    Ok(elapsed) if elapsed < self.min_age => {
                                        stats.skipped_files += 1;
                                        continue;
                                    }
                                    Err(_) => {
                                        stats.skipped_files += 1;
                                        continue;
                                    }
                                    _ => {}
                                }
                            }
                        }

                        let size = metadata.len();
                        if !self.dry_run {
                            if fs::remove_file(&p).is_ok() {
                                stats.files_deleted += 1;
                                stats.bytes_freed += size;
                            } else {
                                stats.errors_count += 1;
                            }
                        } else {
                            stats.files_deleted += 1;
                            stats.bytes_freed += size;
                        }
                    }
                }
            }
        }
    }
}
