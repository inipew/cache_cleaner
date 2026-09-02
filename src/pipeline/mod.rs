use std::path::PathBuf;
use std::time::Duration;

use crate::audit::AuditLogger;
use crate::auth::AuthorizationEngine;
use crate::catalog::TargetCatalog;
use crate::config::DaemonConfig;
use crate::config_pipeline::{EffectiveConfig, ValidatedConfig};
use crate::domain::decision::PolicyDecision;
use crate::domain::result::JobResult;
use crate::domain::types::{AttemptId, ByteCount, GenerationId, UnixTimestamp, WorkerId};
use crate::engine::cancellation::CancellationToken;
use crate::engine::storage::StorageOptimizer;
use crate::error::Result;
use crate::executor::CleanupExecutor;
use crate::hardware::f2fs::F2fsController;
use crate::ipc::protocol::{CleanParams, CleanReport, StorageReport};
use crate::planner::CleanupPlanner;
use crate::policy::PolicyEngine;
use crate::recovery::RecoveryEngine;
use crate::resource::ResourceManager;
use crate::safety::SafetyGate;
use crate::scanner::CandidateScanner;
use crate::scheduler::{JobAdmissionRequest, SchedulerService, TriggerSource};
use crate::store::SqliteStore;
use crate::system::freezer;
use crate::verifier::PostconditionVerifier;
use crate::worker::WorkerPool;

/// Authoritative Clean Pipeline.
/// The single authoritative execution path orchestrating all 10 architecture layers.
pub struct AuthoritativeCleanPipeline {
    config: DaemonConfig,
    catalog: TargetCatalog,
    scanner: CandidateScanner,
    safety: SafetyGate,
    policy: PolicyEngine,
    planner: CleanupPlanner,
    auth: AuthorizationEngine,
    executor: CleanupExecutor,
    verifier: PostconditionVerifier,
    store: SqliteStore,
    audit_logger: AuditLogger,
    resource_mgr: ResourceManager,
    recovery: RecoveryEngine,
    scheduler: SchedulerService,
    worker_pool: WorkerPool,
    f2fs: F2fsController,
    config_generation: GenerationId,
}

impl AuthoritativeCleanPipeline {
    pub fn new(config: DaemonConfig) -> Result<Self> {
        let store = SqliteStore::default_store()?;
        let catalog = TargetCatalog::new();
        let verifier = PostconditionVerifier::new();
        let recovery = RecoveryEngine::new();

        // Perform startup crash reconciliation
        let _ = recovery.reconcile_startup_crashes(&store, &catalog, &verifier);

        let scheduler = SchedulerService::new(store.clone());
        let worker_pool = WorkerPool::new(store.clone(), WorkerId(1));
        let audit_logger = AuditLogger::default_logger().unwrap_or_else(|_| {
            AuditLogger::open_or_create(&PathBuf::from("/data/adb/cleaner/audit/audit.jsonl"))
                .unwrap_or_else(|_| AuditLogger::open_or_create(&std::env::temp_dir().join("audit.jsonl")).unwrap())
        });

        Ok(Self {
            config,
            catalog,
            scanner: CandidateScanner::new(),
            safety: SafetyGate::new(),
            policy: PolicyEngine::new(),
            planner: CleanupPlanner::new(),
            auth: AuthorizationEngine::new(),
            executor: CleanupExecutor::new(),
            verifier,
            store,
            audit_logger,
            resource_mgr: ResourceManager::default(),
            recovery,
            scheduler,
            worker_pool,
            f2fs: F2fsController::discover(),
            config_generation: GenerationId::INITIAL,
        })
    }

    /// Primary execution endpoint. Admits, claims, scans, plans, authorizes, mutates, and audits a clean job.
    pub fn execute(&mut self, params: &CleanParams, cancel_token: &CancellationToken) -> Result<CleanReport> {
        // 1. Refresh catalog and discover targets
        self.catalog.discover_all_targets();
        let snapshot = self.catalog.take_snapshot();

        // 2. Admit job into scheduler
        let trigger = if params.deep {
            TriggerSource::ManualIpc
        } else {
            TriggerSource::PeriodicMaintenance
        };

        let admission = self.scheduler.admit_job(
            trigger,
            params.deep,
            params.trim,
            params.dry_run,
            snapshot.generation,
            self.config_generation,
        )?;

        // 3. Claim attempt lease in WorkerPool
        let attempt_id = self.worker_pool.claim_attempt(admission.job_id, Duration::from_secs(300))?;

        let result = self.run_pipeline_job(&admission, attempt_id, &snapshot, cancel_token);

        match &result {
            Ok(report) => {
                let _ = self.store.update_job_state(admission.job_id, "COMPLETED", ByteCount::new(report.storage.total_freed_bytes));
                let _ = self.worker_pool.finish_attempt(attempt_id, admission.job_id, "SUCCESS");
            }
            Err(e) => {
                let _ = self.store.update_job_state(admission.job_id, "FAILED", ByteCount::ZERO);
                let _ = self.worker_pool.finish_attempt(attempt_id, admission.job_id, &format!("FAILED: {}", e));
            }
        }

        result
    }

    fn run_pipeline_job(
        &self,
        admission: &JobAdmissionRequest,
        attempt_id: AttemptId,
        snapshot: &crate::catalog::CatalogSnapshot,
        cancel_token: &CancellationToken,
    ) -> Result<CleanReport> {
        // Build ValidatedConfig & EffectiveConfig
        let val_cfg = ValidatedConfig::from_raw(crate::config_pipeline::RawConfig {
            min_screen_off_secs: Some(self.config.min_screen_off_secs),
            max_soc_temp_c: Some(self.config.max_soc_temp_c),
            max_battery_temp_c: Some(self.config.max_battery_temp_c),
            ..Default::default()
        })?;
        let eff_cfg = EffectiveConfig::new(self.config_generation, val_cfg);

        // Detect frozen UIDs for prioritization
        let frozen_uids = freezer::enumerate_frozen_uids();

        let mut all_permits = Vec::new();
        let now = UnixTimestamp::now();

        // 4. Scan targets via streaming SafeDirHandle with backpressure
        for target in snapshot.targets.values() {
            if cancel_token.is_cancelled() {
                break;
            }

            let candidates = match self.scanner.scan_target_with_resource(target, &self.resource_mgr) {
                Ok(c) => c,
                Err(e) => {
                    log::warn!("Scanning target {} skipped: {}", target.target_id, e);
                    continue;
                }
            };

            for cand in candidates {
                // 5. Validate through SafetyGate
                let safety_validated = match self.safety.validate_candidate(cand, target) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                // 6. Evaluate through PolicyEngine (applying age retention & frozen priority)
                let is_frozen = target.owner_uid > 0 && frozen_uids.contains(&target.owner_uid);
                let decision = self.policy.evaluate_candidate_with_freezer(
                    safety_validated,
                    target,
                    &eff_cfg,
                    now,
                    is_frozen,
                );

                if let PolicyDecision::Allow(permit) = decision {
                    all_permits.push(permit);
                }
            }
        }

        // 7. Construct deterministic hierarchical DAG plan
        let planned_plan = self.planner.build_plan(admission.job_id, snapshot.generation, all_permits);

        // 8. Issue scoped capability grant
        let authorized_plan = self.auth.authorize_plan(
            planned_plan,
            snapshot.generation,
            300,
            admission.config_generation,
        )?;

        // 9. Execute (or Dry Run)
        let job_result = if admission.dry_run {
            JobResult {
                job_id: admission.job_id,
                attempt_id,
                total_reclaimed: authorized_plan.plan.total_estimated_reclaim,
                total_operations: authorized_plan.plan.operations.len(),
                successful_operations: authorized_plan.plan.operations.len(),
                failed_operations: 0,
                skipped_operations: 0,
                duration_ms: 0,
            }
        } else {
            // 10. Execute mutations strictly enforcing durable intent, target lock, and verifier
            self.executor.execute_plan(
                &authorized_plan,
                snapshot,
                attempt_id,
                cancel_token,
                &self.resource_mgr,
                Some(&self.store),
                &self.safety,
                &self.verifier,
            )?
        };

        // 11. Optional F2FS / TRIM maintenance
        let mut fstrim_completed = false;
        if admission.trim && !admission.dry_run {
            let trimmable = StorageOptimizer::discover_trimmable_mounts();
            fstrim_completed = StorageOptimizer::trim_mounts(&trimmable);
        }

        // 12. Persistent Audit Logging
        let _ = self.audit_logger.record_job(&job_result);

        let mut report = CleanReport {
            storage: StorageReport {
                total_freed_bytes: job_result.total_reclaimed.as_u64(),
                deleted_files_count: job_result.successful_operations,
                skipped_files_count: job_result.skipped_operations,
                errors_count: job_result.failed_operations,
                app_cache_bytes: job_result.total_reclaimed.as_u64(),
                oem_logs_bytes: 0,
                crash_dumps_bytes: 0,
                temp_apks_bytes: 0,
                frozen_apps_cleaned: 0,
                active_apps_cleaned: 0,
            },
            memory: crate::ipc::protocol::MemoryReport::default(),
            trim: crate::ipc::protocol::TrimReport {
                fstrim_completed,
                trimmed_mounts: Vec::new(),
            },
            optimization: crate::ipc::protocol::OptimizationReport::default(),
            duration_ms: job_result.duration_ms,
            plan_entries: None,
            cancel_reason: None,
            ..Default::default()
        };
        report.sync_compat_fields();

        Ok(report)
    }

    pub fn store(&self) -> &SqliteStore {
        &self.store
    }

    pub fn scheduler(&self) -> &SchedulerService {
        &self.scheduler
    }

    pub fn catalog(&self) -> &TargetCatalog {
        &self.catalog
    }

    pub fn recovery(&self) -> &RecoveryEngine {
        &self.recovery
    }

    pub fn f2fs(&self) -> &F2fsController {
        &self.f2fs
    }

    pub fn update_config(&mut self, config: DaemonConfig) {
        self.config = config;
        self.config_generation = GenerationId(self.config_generation.0.wrapping_add(1));
    }
}
