use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

use crate::domain::types::TargetId;
use crate::error::{CleanerError, Result};
use crate::util::rate_limiter::{ThrottleMode, TokenBucketRateLimiter};

pub const DEFAULT_MAX_CONCURRENT_FDS: usize = 64;

/// RAII permit for an open File Descriptor, returning it to the pool on drop.
#[derive(Debug)]
pub struct FdPermit {
    counter: Arc<AtomicUsize>,
}

impl Drop for FdPermit {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::SeqCst);
    }
}

/// RAII permit for an exclusive Target mutation lock.
#[derive(Debug)]
pub struct TargetLockPermit {
    target_id: TargetId,
    lock_map: Arc<RwLock<HashSet<TargetId>>>,
}

impl Drop for TargetLockPermit {
    fn drop(&mut self) {
        if let Ok(mut set) = self.lock_map.write() {
            set.remove(&self.target_id);
        }
    }
}

/// Resource Manager regulating global file descriptor allocations, target exclusivity, and rate limits.
pub struct ResourceManager {
    max_fds: usize,
    active_fds: Arc<AtomicUsize>,
    locked_targets: Arc<RwLock<HashSet<TargetId>>>,
    rate_limiter: Arc<TokenBucketRateLimiter>,
}

impl std::fmt::Debug for ResourceManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourceManager")
            .field("max_fds", &self.max_fds)
            .field("active_fds", &self.active_fds)
            .field("locked_targets", &self.locked_targets)
            .finish()
    }
}

impl Clone for ResourceManager {
    fn clone(&self) -> Self {
        Self {
            max_fds: self.max_fds,
            active_fds: Arc::clone(&self.active_fds),
            locked_targets: Arc::clone(&self.locked_targets),
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
            max_fds,
            active_fds: Arc::new(AtomicUsize::new(0)),
            locked_targets: Arc::new(RwLock::new(HashSet::new())),
            rate_limiter: Arc::new(TokenBucketRateLimiter::new(initial_mode)),
        }
    }

    /// Acquires an FD permit from the bounded pool. Fails closed if pool is exhausted.
    pub fn acquire_fd_permit(&self) -> Result<FdPermit> {
        let current = self.active_fds.fetch_add(1, Ordering::SeqCst);
        if current >= self.max_fds {
            self.active_fds.fetch_sub(1, Ordering::SeqCst);
            return Err(CleanerError::SafetyViolation(format!(
                "Resource pool exhausted: maximum concurrent FD limit ({}) reached",
                self.max_fds
            )));
        }

        Ok(FdPermit {
            counter: Arc::clone(&self.active_fds),
        })
    }

    /// Acquires an exclusive target mutation lock. Fails closed if target is already locked.
    pub fn acquire_target_lock(&self, target_id: &TargetId) -> Result<TargetLockPermit> {
        let mut set = self.locked_targets.write().map_err(|_| {
            CleanerError::SafetyViolation("Target lock poisoning".into())
        })?;

        if set.contains(target_id) {
            return Err(CleanerError::SafetyViolation(format!(
                "Concurrent mutation collision: Target {} is already locked by another operation",
                target_id
            )));
        }

        set.insert(target_id.clone());

        Ok(TargetLockPermit {
            target_id: target_id.clone(),
            lock_map: Arc::clone(&self.locked_targets),
        })
    }

    /// Throttle mutation operation using the token bucket rate limiter.
    pub fn throttle_mutation(&self) {
        let _ = self.rate_limiter.acquire();
    }

    pub fn set_throttle_mode(&self, mode: ThrottleMode) {
        self.rate_limiter.set_mode(mode);
    }

    pub fn active_fd_count(&self) -> usize {
        self.active_fds.load(Ordering::SeqCst)
    }

    pub fn is_target_locked(&self, target_id: &TargetId) -> bool {
        self.locked_targets.read().map(|set| set.contains(target_id)).unwrap_or(false)
    }
}
