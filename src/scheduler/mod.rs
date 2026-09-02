use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use serde::{Deserialize, Serialize};

use crate::domain::types::{GenerationId, JobId, UnixTimestamp};
use crate::error::{CleanerError, Result};
use crate::store::SqliteStore;

pub const MAX_QUEUED_JOBS: usize = 32;

/// Trigger source initiating a cleanup evaluation or job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TriggerSource {
    ManualIpc,
    ScreenOff,
    DeepIdle,
    MemoryPressurePsi,
    PeriodicMaintenance,
}

/// Request admitted into the scheduler queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobAdmissionRequest {
    pub job_id: JobId,
    pub source: TriggerSource,
    pub deep: bool,
    pub trim: bool,
    pub dry_run: bool,
    pub catalog_generation: GenerationId,
    pub config_generation: GenerationId,
    pub admitted_at: UnixTimestamp,
}

/// Dedicated Scheduler Service managing work admission, queue deduplication, and trigger evaluation.
#[derive(Debug, Clone)]
pub struct SchedulerService {
    queue: Arc<Mutex<VecDeque<JobAdmissionRequest>>>,
    store: SqliteStore,
    job_counter: Arc<Mutex<u64>>,
}

impl SchedulerService {
    pub fn new(store: SqliteStore) -> Self {
        let next_seed = store.get_max_job_id().unwrap_or(0) + 1;
        Self {
            queue: Arc::new(Mutex::new(VecDeque::new())),
            store,
            job_counter: Arc::new(Mutex::new(next_seed)),
        }
    }

    /// Admits a new job request into the scheduler queue, enforcing bounds and registering it in SQLite.
    pub fn admit_job(
        &self,
        source: TriggerSource,
        deep: bool,
        trim: bool,
        dry_run: bool,
        catalog_gen: GenerationId,
        config_gen: GenerationId,
    ) -> Result<JobAdmissionRequest> {
        let mut queue = self.queue.lock().map_err(|_| {
            CleanerError::Internal("Scheduler queue lock poisoned".into())
        })?;

        if queue.len() >= MAX_QUEUED_JOBS {
            return Err(CleanerError::ResourceExhausted(format!(
                "Scheduler queue full ({} items)",
                MAX_QUEUED_JOBS
            )));
        }

        let job_id = {
            let mut counter = self.job_counter.lock().unwrap();
            let id = JobId(*counter);
            *counter += 1;
            id
        };

        // Register in durable SQLite store
        self.store.register_job(job_id, &format!("{:?}", source), catalog_gen, config_gen)?;

        let request = JobAdmissionRequest {
            job_id,
            source,
            deep,
            trim,
            dry_run,
            catalog_generation: catalog_gen,
            config_generation: config_gen,
            admitted_at: UnixTimestamp::now(),
        };

        queue.push_back(request.clone());
        log::info!("Job {} admitted from source {:?}", job_id, source);

        Ok(request)
    }

    /// Pops the next admitted job request for execution.
    pub fn pop_next_job(&self) -> Option<JobAdmissionRequest> {
        let mut queue = self.queue.lock().ok()?;
        queue.pop_front()
    }

    pub fn queued_count(&self) -> usize {
        self.queue.lock().map(|q| q.len()).unwrap_or(0)
    }
}
