use std::sync::atomic::{AtomicU64, Ordering};
use serde::{Deserialize, Serialize};

use crate::domain::candidate::Candidate;
use crate::domain::target::TargetDescriptor;
use crate::domain::types::{ByteCount, CandidateId, RelativePath, UnixTimestamp};
use crate::error::Result;
use crate::fs::SafeDirHandle;
use crate::resource::ResourceManager;

pub const DEFAULT_SCAN_CHUNK_SIZE: usize = 500;
pub const MAX_CANDIDATES_PER_TARGET: usize = 50_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScanStatus {
    Complete,
    Partial,
    Failed,
}

#[derive(Debug, Clone)]
pub struct ScanResult {
    pub candidates: Vec<Candidate>,
    pub status: ScanStatus,
    pub errors: Vec<String>,
}

impl ScanResult {
    pub fn new(candidates: Vec<Candidate>, status: ScanStatus, errors: Vec<String>) -> Self {
        Self {
            candidates,
            status,
            errors,
        }
    }
}

impl std::ops::Deref for ScanResult {
    type Target = [Candidate];
    fn deref(&self) -> &Self::Target {
        &self.candidates
    }
}

impl IntoIterator for ScanResult {
    type Item = Candidate;
    type IntoIter = std::vec::IntoIter<Candidate>;
    fn into_iter(self) -> Self::IntoIter {
        self.candidates.into_iter()
    }
}

/// Candidate Scanner discovering files and sub-directories within registered targets.
/// Traverses filesystem FD-relatively using `SafeDirHandle` and `RawDir`, enforcing resource permits and backpressure.
#[derive(Debug)]
pub struct CandidateScanner {
    candidate_id_counter: AtomicU64,
}

impl Default for CandidateScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl CandidateScanner {
    pub fn new() -> Self {
        Self {
            candidate_id_counter: AtomicU64::new(1),
        }
    }

    fn next_candidate_id(&self) -> CandidateId {
        CandidateId(self.candidate_id_counter.fetch_add(1, Ordering::Relaxed))
    }

    /// Scans a target descriptor recursively and collects candidates into memory using SafeDirHandle.
    pub fn scan_target(&self, target: &TargetDescriptor) -> Result<ScanResult> {
        let default_res = ResourceManager::default();
        self.scan_target_with_resource(target, &default_res)
    }

    /// Scans a target descriptor with explicit ResourceManager governor and error tracking.
    pub fn scan_target_with_resource(
        &self,
        target: &TargetDescriptor,
        resource_mgr: &ResourceManager,
    ) -> Result<ScanResult> {
        let mut candidates = Vec::new();
        let mut errors = Vec::new();
        let root_permit = resource_mgr.acquire_fd_permit().ok();

        let root_handle = match SafeDirHandle::open_root_with_permit(&target.base_path, root_permit) {
            Ok(h) => h,
            Err(e) => {
                if !target.base_path.exists() {
                    return Ok(ScanResult::new(candidates, ScanStatus::Complete, errors));
                }
                errors.push(format!("Failed to open target root {}: {}", target.base_path.display(), e));
                return Ok(ScanResult::new(candidates, ScanStatus::Failed, errors));
            }
        };

        let mut hit_limit = false;
        self.scan_safe_recursive(
            target,
            &root_handle,
            &RelativePath::empty(),
            resource_mgr,
            &mut candidates,
            &mut errors,
            &mut hit_limit,
            0,
            16,
        )?;

        let status = if hit_limit {
            ScanStatus::Partial
        } else if !errors.is_empty() && candidates.is_empty() {
            ScanStatus::Failed
        } else if !errors.is_empty() {
            ScanStatus::Partial
        } else {
            ScanStatus::Complete
        };

        Ok(ScanResult::new(candidates, status, errors))
    }

    /// Scans a target descriptor with chunked streaming, SafeDirHandle FD-safety, and resource governor permits.
    pub fn scan_target_streaming<F>(
        &self,
        target: &TargetDescriptor,
        resource_mgr: &ResourceManager,
        chunk_size: usize,
        mut on_chunk: F,
    ) -> Result<()>
    where
        F: FnMut(Vec<Candidate>) -> Result<bool>, // returns false to cancel/preempt early
    {
        let root_permit = resource_mgr.acquire_fd_permit().ok();
        let root_handle = match SafeDirHandle::open_root_with_permit(&target.base_path, root_permit) {
            Ok(h) => h,
            Err(_) => return Ok(()),
        };

        let mut buffer = Vec::with_capacity(chunk_size);
        self.scan_safe_recursive_streaming(
            target,
            &root_handle,
            &RelativePath::empty(),
            resource_mgr,
            &mut buffer,
            chunk_size,
            &mut on_chunk,
            0,
            16,
        )?;

        // Flush remaining items
        if !buffer.is_empty() {
            let _ = on_chunk(buffer)?;
        }

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn scan_safe_recursive(
        &self,
        target: &TargetDescriptor,
        current_handle: &SafeDirHandle,
        rel_prefix: &RelativePath,
        resource_mgr: &ResourceManager,
        candidates: &mut Vec<Candidate>,
        errors: &mut Vec<String>,
        hit_limit: &mut bool,
        depth: usize,
        max_depth: usize,
    ) -> Result<()> {
        if depth >= max_depth {
            return Ok(());
        }
        if candidates.len() >= MAX_CANDIDATES_PER_TARGET {
            *hit_limit = true;
            return Ok(());
        }

        let entries = match current_handle.read_entries_fd() {
            Ok(e) => e,
            Err(e) => {
                errors.push(format!("Failed to read directory {}: {}", rel_prefix.as_str(), e));
                return Ok(());
            }
        };

        for entry in entries {
            if candidates.len() >= MAX_CANDIDATES_PER_TARGET {
                *hit_limit = true;
                break;
            }

            let entry_rel = match rel_prefix.join(&entry.name) {
                Some(p) => p,
                None => continue,
            };

            if entry.is_dir && !entry.is_symlink {
                let candidate = Candidate {
                    candidate_id: self.next_candidate_id(),
                    target_id: target.target_id.clone(),
                    rel_path: entry_rel.clone(),
                    identity: entry.identity,
                    size_bytes: ByteCount::ZERO,
                    mtime: UnixTimestamp::from_secs(entry.mtime_secs),
                    atime: None,
                    is_dir: true,
                    is_symlink: false,
                };
                candidates.push(candidate);

                // Acquire bounded permit for recursive subdirectory exploration
                if let Ok(child_permit) = resource_mgr.acquire_fd_permit() {
                    if let Ok(child_handle) = current_handle.open_child_dir_with_permit(&entry.name, Some(child_permit)) {
                        self.scan_safe_recursive(
                            target,
                            &child_handle,
                            &entry_rel,
                            resource_mgr,
                            candidates,
                            errors,
                            hit_limit,
                            depth + 1,
                            max_depth,
                        )?;
                    }
                }
            } else {
                let candidate = Candidate {
                    candidate_id: self.next_candidate_id(),
                    target_id: target.target_id.clone(),
                    rel_path: entry_rel,
                    identity: entry.identity,
                    size_bytes: ByteCount::new(entry.size_bytes),
                    mtime: UnixTimestamp::from_secs(entry.mtime_secs),
                    atime: None,
                    is_dir: false,
                    is_symlink: entry.is_symlink,
                };
                candidates.push(candidate);
            }
        }

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn scan_safe_recursive_streaming<F>(
        &self,
        target: &TargetDescriptor,
        current_handle: &SafeDirHandle,
        rel_prefix: &RelativePath,
        resource_mgr: &ResourceManager,
        buffer: &mut Vec<Candidate>,
        chunk_size: usize,
        on_chunk: &mut F,
        depth: usize,
        max_depth: usize,
    ) -> Result<bool>
    where
        F: FnMut(Vec<Candidate>) -> Result<bool>,
    {
        if depth >= max_depth {
            return Ok(true);
        }

        let entries = match current_handle.read_entries_fd() {
            Ok(e) => e,
            Err(_) => return Ok(true),
        };

        for entry in entries {
            let entry_rel = match rel_prefix.join(&entry.name) {
                Some(p) => p,
                None => continue,
            };

            let candidate = if entry.is_dir && !entry.is_symlink {
                Candidate {
                    candidate_id: self.next_candidate_id(),
                    target_id: target.target_id.clone(),
                    rel_path: entry_rel.clone(),
                    identity: entry.identity,
                    size_bytes: ByteCount::ZERO,
                    mtime: UnixTimestamp::from_secs(entry.mtime_secs),
                    atime: None,
                    is_dir: true,
                    is_symlink: false,
                }
            } else {
                Candidate {
                    candidate_id: self.next_candidate_id(),
                    target_id: target.target_id.clone(),
                    rel_path: entry_rel.clone(),
                    identity: entry.identity,
                    size_bytes: ByteCount::new(entry.size_bytes),
                    mtime: UnixTimestamp::from_secs(entry.mtime_secs),
                    atime: None,
                    is_dir: false,
                    is_symlink: entry.is_symlink,
                }
            };

            buffer.push(candidate);
            if buffer.len() >= chunk_size {
                let chunk = std::mem::replace(buffer, Vec::with_capacity(chunk_size));
                let should_continue = on_chunk(chunk)?;
                if !should_continue {
                    return Ok(false);
                }
            }

            if entry.is_dir && !entry.is_symlink {
                if let Ok(child_permit) = resource_mgr.acquire_fd_permit() {
                    if let Ok(child_handle) = current_handle.open_child_dir_with_permit(&entry.name, Some(child_permit)) {
                        let should_continue = self.scan_safe_recursive_streaming(
                            target,
                            &child_handle,
                            &entry_rel,
                            resource_mgr,
                            buffer,
                            chunk_size,
                            on_chunk,
                            depth + 1,
                            max_depth,
                        )?;
                        if !should_continue {
                            return Ok(false);
                        }
                    }
                }
            }
        }

        Ok(true)
    }
}
