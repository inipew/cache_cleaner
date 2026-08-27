use std::path::PathBuf;
use std::sync::Arc;

use crate::config::DaemonConfig;
use crate::ipc::server::IpcServer;
use crate::system::cgroup::migrate_to_background_cgroup;
use crate::system::governor::set_idle_priorities;
use crate::system::pidfile::PidLock;

pub use crate::system::daemon_state::{DaemonContext, DaemonRuntimeState, DaemonState};
pub use crate::system::event_loop::{run_epoll_loop, run_fallback_loop};

/// `DaemonRunner` manages the process lifecycle, PID lock, OS prioritization, and launches the event reactor.
pub struct DaemonRunner {

    ctx: Arc<DaemonContext>,
}

impl DaemonRunner {
    #[must_use]
    pub fn new(config: DaemonConfig, active_config_path: Option<PathBuf>) -> Self {
        Self {
            ctx: Arc::new(DaemonContext::new(config, active_config_path)),
        }
    }

    #[must_use]
    pub fn context(&self) -> Arc<DaemonContext> {
        self.ctx.clone()
    }

    pub fn run(&mut self) {
        log::info!("Starting Android Cache Cleaner Daemon...");

        #[cfg(unix)]
        unsafe {
            // Set safe umask (preserves init / supervisor session hierarchy)
            libc::umask(0o027);
        }

        // 1. Acquire exclusive PID file lock (Guarantees zero duplicate instances)
        let _pid_lock = match PidLock::acquire() {
            Ok(lock) => {
                log::info!("Daemon PID lock acquired at {}", lock.path().display());
                lock
            }
            Err(e) => {
                log::error!("Cannot start daemon: {e}");
                eprintln!("[!] Cannot start daemon: {e}");
                return;
            }
        };

        // 2. Set background Cgroup and CPU/IO low priorities
        let cgroup_summary = migrate_to_background_cgroup();
        log::info!(
            "Cgroup isolation applied: version={:?}, fully_migrated={}, controllers={}",
            cgroup_summary.version,
            cgroup_summary.fully_migrated,
            cgroup_summary.migrations.len()
        );
        set_idle_priorities();

        // 3. Initialize IPC Server
        let (socket_path, abstract_socket_name) = {
            let cfg = self.ctx.config.read().unwrap_or_else(std::sync::PoisonError::into_inner);
            (cfg.socket_path.clone(), cfg.abstract_socket_name.clone())
        };

        let ipc_server = match IpcServer::bind(&socket_path, &abstract_socket_name) {
            Ok(server) => Some(server),
            Err(e) => {
                log::warn!(
                    "Failed to initialize IPC server: {e}. Continuing without IPC."
                );
                None
            }
        };


        // Trim startup heap allocations back to the kernel before entering event loop
        crate::util::trim_heap_memory();

        // 4. Linux Kernel Epoll Event Loop
        #[cfg(unix)]
        run_epoll_loop(&self.ctx, ipc_server.as_ref());

        #[cfg(not(unix))]
        run_fallback_loop(&self.ctx, ipc_server.as_ref());

        // 5. Graceful Cleanup and Termination
        self.ctx.perform_shutdown_cleanup();
        log::info!("Cache Cleaner Daemon terminated completely and safely.");
    }
}
