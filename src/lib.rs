pub mod cli;
pub mod config;
pub mod engine;
pub mod error;
pub mod hardware;
pub mod ipc;
pub mod platform;
pub mod system;
pub mod util;

pub use config::DaemonConfig;
pub use engine::{CancellationToken, CleanEngine};
pub use error::{CleanerError, Result};
pub use ipc::client::IpcClient;
pub use ipc::protocol::{CleanParams, CleanReport, Command, DaemonStatus, Response, ResponseData};
pub use ipc::server::IpcServer;
pub use platform::{check_encryption_state, enumerate_users, get_selinux_mode, AndroidSystemInfo};
pub use system::{DaemonContext, DaemonRunner, DaemonState};
pub use util::{format_bytes, init_logger};
