#[cfg(unix)]
use rustix::fs::{openat, statat, unlinkat, AtFlags, FileType, Mode, OFlags, RawDir, CWD};
use std::fs;
#[cfg(unix)]
use std::mem::MaybeUninit;
use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::engine::cancellation::CancellationToken;
use crate::engine::rules::{JunkType, RuleEngine};

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

pub struct DirectoryWalker<'a> {
    rule_engine: &'a RuleEngine,
    cancel_token: &'a CancellationToken,
    min_age: Duration,
    dry_run: bool,
    frozen_uids: Option<&'a std::collections::HashSet<u32>>,
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
        }
    }


    pub fn with_frozen_uids(
        mut self,
        frozen_uids: Option<&'a std::collections::HashSet<u32>>,
    ) -> Self {
        self.frozen_uids = frozen_uids;
        self
    }

    /// Recursively purge junk files inside a directory tree safely using zero-copy getdents64 & fd-relative syscalls
    pub fn clean_directory(&self, root: &Path) -> WalkStats {
        let mut stats = WalkStats::default();

        let resolved_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        if !resolved_root.exists() {
            return stats;
        }

        self.walk_internal(&resolved_root, &mut stats, 0);
        stats
    }

    /// Clean crash dump directory retaining the newest N files
    pub fn clean_crash_dumps_directory(&self, dir: &Path, keep_count: usize) -> WalkStats {
        let mut stats = WalkStats::default();

        let resolved_dir = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        if !resolved_dir.exists() {
            return stats;
        }

        let mut crash_files: Vec<(std::path::PathBuf, u64, SystemTime)> = Vec::new();

        #[cfg(unix)]
        {
            if let Ok(dir_fd) = openat(
                CWD,
                &resolved_dir,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::empty(),
            ) {
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
                                if let Ok(st) = statat(&dir_fd, name_str, AtFlags::SYMLINK_NOFOLLOW)
                                {
                                    let mtime =
                                        SystemTime::UNIX_EPOCH + st_mtime_to_duration(st.st_mtime);
                                    let path = resolved_dir.join(name_str);
                                    crash_files.push((path, st.st_size as u64, mtime));
                                }
                            }
                        }
                    }
                }
            }
        }

        #[cfg(not(unix))]
        {
            if let Ok(entries) = fs::read_dir(&resolved_dir) {
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
            }
        }

        // Sort descending by modified time (newest first)
        crash_files.sort_by_key(|b| std::cmp::Reverse(b.2));



        // Skip the newest `keep_count` files, delete the older ones
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

        stats
    }

    fn walk_internal(&self, dir: &Path, stats: &mut WalkStats, depth: usize) {
        if depth > 20 || self.cancel_token.is_cancelled() {
            return;
        }

        #[cfg(unix)]
        {
            // Open directory with O_NOFOLLOW to avoid symlink directory escape
            let dir_fd = match openat(
                CWD,
                dir,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::empty(),
            ) {
                Ok(fd) => fd,
                Err(err) => {
                    // Gracefully skip encrypted/inaccessible FBE directories (e.g. ENOKEY / EACCES)
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
                        // Non-UTF8 may indicate locked raw FBE ciphertext; skip safely
                        stats.skipped_files += 1;
                        continue;
                    }
                };

                let path = dir.join(name_str);
                let file_type = entry.file_type();

                let junk_type = self.rule_engine.classify_path(&path);

                if junk_type == JunkType::Ignored {
                    if file_type == FileType::Directory {
                        #[cfg(unix)]
                        if depth == 0 {
                            if let Ok(st) = statat(&dir_fd, name_str, AtFlags::SYMLINK_NOFOLLOW) {
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
                        }
                        self.walk_internal(&path, stats, depth + 1);
                    }
                    continue;
                }

                // It matched a junk category!
                if file_type == FileType::Symlink {
                    if !self.dry_run {
                        if unlinkat(&dir_fd, name_str, AtFlags::empty()).is_ok() {
                            stats.files_deleted += 1;
                        } else {
                            stats.errors_count += 1;
                        }
                    } else {
                        stats.files_deleted += 1;
                    }
                } else if file_type == FileType::RegularFile {
                    if let Ok(st) = statat(&dir_fd, name_str, AtFlags::SYMLINK_NOFOLLOW) {
                        let file_size = st.st_size as u64;

                        // Age check
                        if self.min_age.as_secs() > 0 {
                            let mtime = SystemTime::UNIX_EPOCH + st_mtime_to_duration(st.st_mtime);
                            if let Ok(elapsed) = now.duration_since(mtime) {
                                if elapsed < self.min_age {
                                    continue;
                                }
                            }
                        }

                        if !self.dry_run {
                            if unlinkat(&dir_fd, name_str, AtFlags::empty()).is_ok() {
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
                } else if file_type == FileType::Directory {
                    // For junk folders (like cache/ or .thumbnails/), purge all files inside
                    self.purge_folder_contents(&path, stats, depth + 1);
                }
            }
        }

        #[cfg(not(unix))]
        {
            let dir_meta = match fs::symlink_metadata(dir) {
                Ok(m) => m,
                Err(_) => return,
            };

            if dir_meta.file_type().is_symlink() {
                return;
            }

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

                let junk_type = self.rule_engine.classify_path(&path);

                if junk_type == JunkType::Ignored {
                    if file_type.is_dir() && !file_type.is_symlink() {
                        self.walk_internal(&path, stats, depth + 1);
                    }
                    continue;
                }

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
                                if let Ok(elapsed) = now.duration_since(modified) {
                                    if elapsed < self.min_age {
                                        continue;
                                    }
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
                    self.purge_folder_contents(&path, stats, depth + 1);
                }
            }
        }
    }

    fn purge_folder_contents(&self, folder: &Path, stats: &mut WalkStats, depth: usize) {
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

                if self.rule_engine.classify_path(&p) == JunkType::Ignored {
                    if file_type == FileType::Directory {
                        self.walk_internal(&p, stats, depth + 1);
                    }
                    continue;
                }

                if file_type == FileType::Symlink {
                    if !self.dry_run {
                        if unlinkat(&dir_fd, name_str, AtFlags::empty()).is_ok() {
                            stats.files_deleted += 1;
                        } else {
                            stats.errors_count += 1;
                        }
                    } else {
                        stats.files_deleted += 1;
                    }
                } else if file_type == FileType::Directory {
                    self.purge_folder_contents(&p, stats, depth + 1);
                    if !self.dry_run {
                        let _ = unlinkat(&dir_fd, name_str, AtFlags::REMOVEDIR);
                    }
                } else if file_type == FileType::RegularFile {
                    if let Ok(st) = statat(&dir_fd, name_str, AtFlags::SYMLINK_NOFOLLOW) {
                        if self.min_age.as_secs() > 0 {
                            let mtime = SystemTime::UNIX_EPOCH + st_mtime_to_duration(st.st_mtime);
                            if let Ok(elapsed) = now.duration_since(mtime) {
                                if elapsed < self.min_age {
                                    continue;
                                }
                            }
                        }

                        let size = st.st_size as u64;
                        if !self.dry_run {
                            if unlinkat(&dir_fd, name_str, AtFlags::empty()).is_ok() {
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

        #[cfg(not(unix))]
        {
            if let Ok(meta) = fs::symlink_metadata(folder) {
                if meta.file_type().is_symlink() {
                    if !self.dry_run {
                        let _ = fs::remove_file(folder);
                    }
                    stats.files_deleted += 1;
                    return;
                }
            }

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

                if self.rule_engine.classify_path(&p) == JunkType::Ignored {
                    if file_type.is_dir() && !file_type.is_symlink() {
                        self.walk_internal(&p, stats, depth + 1);
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
                    self.purge_folder_contents(&p, stats, depth + 1);
                    if !self.dry_run {
                        let _ = fs::remove_dir(&p);
                    }
                } else if file_type.is_file() {
                    if let Ok(metadata) = entry.metadata() {
                        if self.min_age.as_secs() > 0 {
                            if let Ok(modified) = metadata.modified() {
                                if let Ok(elapsed) = now.duration_since(modified) {
                                    if elapsed < self.min_age {
                                        continue;
                                    }
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
