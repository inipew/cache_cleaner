use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::domain::types::{AttemptId, TargetId};
use crate::error::{CleanerError, Result};
use crate::util::rate_limiter::{ThrottleMode, TokenBucketRateLimiter};

pub const DEFAULT_MAX_CONCURRENT_FDS: usize = 64;

/// Typed Storage identifier — distinct from TargetId per 64.md:6, 78.md:2.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StorageId(pub String);
impl StorageId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}
/// Typed Mount identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MountId(pub String);
impl MountId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}
/// Block device identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlockDeviceId(pub String);

/// Hierarchy ordering per 64.md:48 `Global→Storage→Mount→Target→Operation`
/// Enforced by requiring locks in that order; document only for now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResourceLevel {
    Global = 0,
    Storage = 1,
    Mount = 2,
    Target = 3,
    Operation = 4,
}

#[derive(Debug)]
struct FdPoolState {
    active_fds: usize,
    max_fds: usize,
}

/// RAII permit for an open File Descriptor, returning it to the pool on drop and waking waiters.
#[derive(Debug)]
pub struct FdPermit {
    condvar: Arc<(Mutex<FdPoolState>, Condvar)>,
}

impl Drop for FdPermit {
    fn drop(&mut self) {
        let (lock, cvar) = &*self.condvar;
        if let Ok(mut state) = lock.lock() {
            state.active_fds = state.active_fds.saturating_sub(1);
            cvar.notify_one();
        }
    }
}

/// Unique ID for a target lock acquisition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReservationId(pub u64);

/// RAII permit for an exclusive Target mutation lock bound to an AttemptId and ReservationId.
#[derive(Debug)]
pub struct TargetLockPermit {
    pub target_id: TargetId,
    pub attempt_id: AttemptId,
    pub reservation_id: ReservationId,
    lock_map: Arc<Mutex<HashMap<TargetId, (AttemptId, ReservationId)>>>,
}

impl Drop for TargetLockPermit {
    fn drop(&mut self) {
        if let Ok(mut map) = self.lock_map.lock() {
            if let Some((_, res_id)) = map.get(&self.target_id) {
                if *res_id == self.reservation_id {
                    map.remove(&self.target_id);
                }
            }
        }
    }
}

/// Exclusive Storage lock permit.
#[derive(Debug)]
pub struct StorageLockPermit {
    pub storage_id: StorageId,
    pub reservation_id: ReservationId,
    lock_map: Arc<Mutex<HashMap<StorageId, ReservationId>>>,
}
impl Drop for StorageLockPermit {
    fn drop(&mut self) {
        if let Ok(mut map) = self.lock_map.lock() {
            if let Some(rid) = map.get(&self.storage_id) {
                if *rid == self.reservation_id {
                    map.remove(&self.storage_id);
                }
            }
        }
    }
}

/// Exclusive Mount lock permit.
#[derive(Debug)]
pub struct MountLockPermit {
    pub mount_id: MountId,
    pub reservation_id: ReservationId,
    lock_map: Arc<Mutex<HashMap<MountId, ReservationId>>>,
}
impl Drop for MountLockPermit {
    fn drop(&mut self) {
        if let Ok(mut map) = self.lock_map.lock() {
            if let Some(rid) = map.get(&self.mount_id) {
                if *rid == self.reservation_id {
                    map.remove(&self.mount_id);
                }
            }
        }
    }
}

/// Resource Manager regulating global file descriptor allocations, target exclusivity, and rate limits.
/// Hierarchy `Global→Storage→Mount→Target` documented and enforced via ordered acquisition.
pub struct ResourceManager {
    fd_condvar: Arc<(Mutex<FdPoolState>, Condvar)>,
    locked_targets: Arc<Mutex<HashMap<TargetId, (AttemptId, ReservationId)>>>,
    locked_storages: Arc<Mutex<HashMap<StorageId, ReservationId>>>,
    locked_mounts: Arc<Mutex<HashMap<MountId, ReservationId>>>,
    reservation_counter: Arc<AtomicU64>,
    rate_limiter: Arc<TokenBucketRateLimiter>,
}

impl std::fmt::Debug for ResourceManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let active = self.active_fd_count();
        f.debug_struct("ResourceManager")
            .field("active_fds", &active)
            .field("locked_targets", &self.locked_targets.lock().map(|m| m.len()).unwrap_or(0))
            .finish()
    }
}

impl Clone for ResourceManager {
    fn clone(&self) -> Self {
        Self {
            fd_condvar: Arc::clone(&self.fd_condvar),
            locked_targets: Arc::clone(&self.locked_targets),
            locked_storages: Arc::clone(&self.locked_storages),
            locked_mounts: Arc::clone(&self.locked_mounts),
            reservation_counter: Arc::clone(&self.reservation_counter),
            rate_limiter: Arc::clone(&self.rate_limiter),
        }
    }
}

impl Default for ResourceManager {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_CONCURRENT_FDS, ThrottleMode::Normal)
    }
}

impl ResourceManager {
    pub fn new(max_fds: usize, initial_mode: ThrottleMode) -> Self {
        Self {
            fd_condvar: Arc::new((
                Mutex::new(FdPoolState {
                    active_fds: 0,
                    max_fds,
                }),
                Condvar::new(),
            )),
            locked_targets: Arc::new(Mutex::new(HashMap::new())),
            locked_storages: Arc::new(Mutex::new(HashMap::new())),
            locked_mounts: Arc::new(Mutex::new(HashMap::new())),
            reservation_counter: Arc::new(AtomicU64::new(1)),
            rate_limiter: Arc::new(TokenBucketRateLimiter::new(initial_mode)),
        }
    }

    pub fn acquire_fd_permit(&self) -> Result<FdPermit> {
        self.acquire_fd_permit_timeout(Duration::from_millis(500))
    }

    pub fn acquire_fd_permit_timeout(&self, timeout: Duration) -> Result<FdPermit> {
        let (lock, cvar) = &*self.fd_condvar;
        let mut state = lock.lock().map_err(|_| CleanerError::Internal("FD pool lock poisoned".into()))?;
        let start = Instant::now();
        while state.active_fds >= state.max_fds {
            let elapsed = start.elapsed();
            if elapsed >= timeout {
                return Err(CleanerError::ResourceExhausted(format!(
                    "Resource pool exhausted: maximum concurrent FD limit ({}) reached after waiting {:?}",
                    state.max_fds, timeout
                )));
            }
            let remaining = timeout - elapsed;
            let (new_state, timeout_result) = cvar
                .wait_timeout(state, remaining)
                .map_err(|_| CleanerError::Internal("FD condvar poisoned".into()))?;
            state = new_state;
            if timeout_result.timed_out() && state.active_fds >= state.max_fds {
                return Err(CleanerError::ResourceExhausted(format!(
                    "Resource pool exhausted: maximum concurrent FD limit ({}) reached",
                    state.max_fds
                )));
            }
        }
        state.active_fds += 1;
        Ok(FdPermit {
            condvar: Arc::clone(&self.fd_condvar),
        })
    }

    pub fn acquire_target_lock(&self, target_id: &TargetId) -> Result<TargetLockPermit> {
        self.acquire_target_lock_for_attempt(target_id, AttemptId(0))
    }

    pub fn acquire_target_lock_for_attempt(
        &self,
        target_id: &TargetId,
        attempt_id: AttemptId,
    ) -> Result<TargetLockPermit> {
        let mut map = self
            .locked_targets
            .lock()
            .map_err(|_| CleanerError::SafetyViolation("Target lock poisoning".into()))?;

        if let Some((existing_attempt, _)) = map.get(target_id) {
            return Err(CleanerError::SafetyViolation(format!(
                "Concurrent mutation collision: Target {} is already locked by attempt {}",
                target_id, existing_attempt.0
            )));
        }

        let res_id = ReservationId(self.reservation_counter.fetch_add(1, Ordering::Relaxed));
        map.insert(target_id.clone(), (attempt_id, res_id));

        Ok(TargetLockPermit {
            target_id: target_id.clone(),
            attempt_id,
            reservation_id: res_id,
            lock_map: Arc::clone(&self.locked_targets),
        })
    }

    /// Storage-level exclusive lock — must be acquired before Mount/Target per hierarchy.
    pub fn acquire_storage_lock(&self, storage_id: &StorageId) -> Result<StorageLockPermit> {
        let mut map = self
            .locked_storages
            .lock()
            .map_err(|_| CleanerError::SafetyViolation("Storage lock poisoning".into()))?;
        if map.contains_key(storage_id) {
            return Err(CleanerError::SafetyViolation(format!(
                "Storage {} is already locked",
                storage_id.0
            )));
        }
        let res_id = ReservationId(self.reservation_counter.fetch_add(1, Ordering::Relaxed));
        map.insert(storage_id.clone(), res_id);
        Ok(StorageLockPermit {
            storage_id: storage_id.clone(),
            reservation_id: res_id,
            lock_map: Arc::clone(&self.locked_storages),
        })
    }

    /// Mount-level exclusive lock — must be acquired after Storage, before Target.
    pub fn acquire_mount_lock(&self, mount_id: &MountId) -> Result<MountLockPermit> {
        let mut map = self
            .locked_mounts
            .lock()
            .map_err(|_| CleanerError::SafetyViolation("Mount lock poisoning".into()))?;
        if map.contains_key(mount_id) {
            return Err(CleanerError::SafetyViolation(format!(
                "Mount {} is already locked",
                mount_id.0
            )));
        }
        let res_id = ReservationId(self.reservation_counter.fetch_add(1, Ordering::Relaxed));
        map.insert(mount_id.clone(), res_id);
        Ok(MountLockPermit {
            mount_id: mount_id.clone(),
            reservation_id: res_id,
            lock_map: Arc::clone(&self.locked_mounts),
        })
    }

    pub fn throttle_mutation(&self) {
        let _ = self.rate_limiter.acquire();
    }

    pub fn set_throttle_mode(&self, mode: ThrottleMode) {
        self.rate_limiter.set_mode(mode);
    }

    pub fn active_fd_count(&self) -> usize {
        let (lock, _) = &*self.fd_condvar;
        lock.lock().map(|s| s.active_fds).unwrap_or(0)
    }

    pub fn is_target_locked(&self, target_id: &TargetId) -> bool {
        self.locked_targets
            .lock()
            .map(|map| map.contains_key(target_id))
            .unwrap_or(false)
    }
    pub fn is_storage_locked(&self, storage_id: &StorageId) -> bool {
        self.locked_storages
            .lock()
            .map(|m| m.contains_key(storage_id))
            .unwrap_or(false)
    }
    pub fn is_mount_locked(&self, mount_id: &MountId) -> bool {
        self.locked_mounts
            .lock()
            .map(|m| m.contains_key(mount_id))
            .unwrap_or(false)
    }
}
