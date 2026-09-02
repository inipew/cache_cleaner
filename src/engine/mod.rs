pub mod cancellation;
pub mod memory;
pub mod storage;

pub use cancellation::CancellationToken;
pub use memory::MemoryOptimizer;
pub use storage::StorageOptimizer;

use crate::config::DaemonConfig;
use crate::error::Result;
use crate::ipc::protocol::{CleanParams, CleanReport};
use crate::pipeline::AuthoritativeCleanPipeline;

/// CleanEngine Compatibility Facade.
/// Delegates all execution requests to the AuthoritativeCleanPipeline.
pub struct CleanEngine {
    pipeline: AuthoritativeCleanPipeline,
}

impl CleanEngine {
    pub fn new(config: DaemonConfig) -> Self {
        let pipeline = AuthoritativeCleanPipeline::new(config).expect("Failed to initialize AuthoritativeCleanPipeline");
        Self { pipeline }
    }

    pub fn execute(&mut self, params: &CleanParams, cancel_token: &CancellationToken) -> Result<CleanReport> {
        self.pipeline.execute(params, cancel_token)
    }

    pub fn update_config(&mut self, config: DaemonConfig) {
        self.pipeline.update_config(config);
    }

    pub fn pipeline(&self) -> &AuthoritativeCleanPipeline {
        &self.pipeline
    }

    pub fn pipeline_mut(&mut self) -> &mut AuthoritativeCleanPipeline {
        &mut self.pipeline
    }
}
