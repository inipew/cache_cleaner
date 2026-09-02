use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::domain::types::{AttemptId, JobId, WorkerId};
use crate::error::{CleanerError, Result};
use crate::store::SqliteStore;

/// Execution Worker Pool managing attempt lifecycles and background isolation.
#[derive(Debug, Clone)]
pub struct WorkerPool {
    store: SqliteStore,
    worker_id: WorkerId,
    attempt_counter: Arc<AtomicU64>,
    is_running: Arc<AtomicBool>,
}

impl WorkerPool {
    pub fn new(store: SqliteStore, worker_id: WorkerId) -> Self {
        let next_seed = store.get_max_attempt_id().unwrap_or(0) + 1;
        Self {
            store,
            worker_id,
            attempt_counter: Arc::new(AtomicU64::new(next_seed)),
            is_running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Claims an execution attempt in the durable SQLite store with a lease.
    pub fn claim_attempt(&self, job_id: JobId, lease_duration: Duration) -> Result<AttemptId> {
        let attempt_num = self.attempt_counter.fetch_add(1, Ordering::SeqCst);
        let attempt_id = AttemptId(attempt_num);

        let lease_resource = format!("job-attempt-{}", job_id.0);
        let acquired = self.store.acquire_lease(&lease_resource, self.worker_id, lease_duration.as_secs())?;
        if !acquired {
            return Err(CleanerError::SafetyViolation(format!(
                "Failed to acquire execution lease for job {}: already claimed by another worker",
                job_id
            )));
        }

        self.store.create_attempt(attempt_id, job_id, self.worker_id, lease_duration.as_secs())?;
        log::info!("Worker {} claimed attempt {} for job {}", self.worker_id, attempt_id, job_id);

        Ok(attempt_id)
    }

    /// Finalizes an execution attempt in SQLite.
    pub fn finish_attempt(&self, attempt_id: AttemptId, job_id: JobId, state: &str) -> Result<()> {
        self.store.update_attempt_state(attempt_id, state)?;
        let lease_resource = format!("job-attempt-{}", job_id.0);
        let _ = self.store.release_lease(&lease_resource, self.worker_id);
        Ok(())
    }

    pub fn worker_id(&self) -> WorkerId {
        self.worker_id
    }

    pub fn is_running(&self) -> bool {
        self.is_running.load(Ordering::Relaxed)
    }
}
