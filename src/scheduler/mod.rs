use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use serde::{Deserialize, Serialize};

use crate::domain::types::{CatalogGeneration, ConfigGeneration, JobId, UnixTimestamp};
use crate::error::{CleanerError, Result};
use crate::store::SqliteStore;

pub const MAX_QUEUED_JOBS: usize = 1;

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
    pub catalog_generation: CatalogGeneration,
    pub config_generation: ConfigGeneration,
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

    /// Admits a new job request into the scheduler queue, enforcing single-slot queue with coalescing (Spec 84.md:6).
    pub fn admit_job(
        &self,
        source: TriggerSource,
        deep: bool,
        trim: bool,
        dry_run: bool,
        catalog_gen: CatalogGeneration,
        config_gen: ConfigGeneration,
    ) -> Result<JobAdmissionRequest> {
        let mut queue = self.queue.lock().map_err(|_| {
            CleanerError::Internal("Scheduler queue lock poisoned".into())
        })?;

        // Invariant: Trigger Coalescing (Spec 84.md:6)
        if let Some(existing) = queue.front_mut() {
            existing.deep = existing.deep || deep;
            existing.trim = existing.trim || trim;
            existing.dry_run = existing.dry_run && dry_run;
            log::info!(
                "Coalesced trigger {:?} into existing queued job {} (deep={}, trim={})",
                source, existing.job_id, existing.deep, existing.trim
            );
            return Ok(existing.clone());
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
    pub fn pop_next_job(&self) -> Result<Option<JobAdmissionRequest>> {
        let mut queue = self.queue.lock().map_err(|_| {
            CleanerError::Internal("Scheduler queue lock poisoned".into())
        })?;
        Ok(queue.pop_front())
    }

    pub fn queued_count(&self) -> usize {
        self.queue.lock().map(|q| q.len()).unwrap_or(0)
    }
}
