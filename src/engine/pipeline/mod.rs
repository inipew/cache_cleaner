pub mod app_cache;
pub mod memory_opt;
pub mod storage_opt;
pub mod system_junk;

use crate::config::DaemonConfig;
use crate::engine::cancellation::CancellationToken;
use crate::engine::rules::RuleEngine;
use crate::engine::walker::DirectoryWalker;
use crate::hardware::f2fs::F2fsController;
use crate::ipc::protocol::{CleanParams, CleanReport};
use std::collections::HashSet;

pub use app_cache::AppCacheStage;
pub use memory_opt::MemoryOptStage;
pub use storage_opt::StorageOptStage;
pub use system_junk::SystemJunkStage;

/// Context passed to each pipeline stage during clean execution
pub struct PipelineContext<'a> {
    pub config: &'a DaemonConfig,
    pub params: &'a CleanParams,
    pub cancel_token: &'a CancellationToken,
    pub rule_engine: &'a RuleEngine,
    pub walker: &'a DirectoryWalker<'a>,
    pub f2fs: &'a F2fsController,
    pub frozen_uids: Option<&'a HashSet<u32>>,
}

/// Trait implemented by each distinct cleaning and optimization stage
pub trait CleanStage: Send + Sync {
    fn name(&self) -> &'static str;
    fn execute(&self, ctx: &PipelineContext, report: &mut CleanReport);
}

/// Orchestrator for executing composable clean stages in sequence
pub struct CleanPipeline {
    stages: Vec<Box<dyn CleanStage>>,
}

impl CleanPipeline {
    pub fn build_standard() -> Self {
        Self {
            stages: vec![
                Box::new(AppCacheStage),
                Box::new(SystemJunkStage),
                Box::new(MemoryOptStage),
                Box::new(StorageOptStage),
            ],
        }
    }

    pub fn execute(&self, ctx: &PipelineContext, report: &mut CleanReport) {
        for stage in &self.stages {
            if ctx.cancel_token.is_cancelled() {
                log::warn!("Clean pipeline preempted before stage: {}", stage.name());
                break;
            }
            log::debug!("Executing clean pipeline stage: {}", stage.name());
            stage.execute(ctx, report);
        }
    }
}
