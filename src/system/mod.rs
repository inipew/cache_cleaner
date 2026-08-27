pub mod cgroup;
pub mod daemon_state;
pub mod event_loop;
pub mod freezer;
pub mod governor;
pub mod idle;
pub mod pidfile;
pub mod proc_metrics;
pub mod psi;
pub mod signals;
pub mod watcher;

pub use idle::{
    IdleAssessment, IdleBlocker, IdleContext, IdleManager, IdlePolicy, IdlePositive, IdleState,
    MaintenanceEligibility, SensorReading, SensorStatus, ThermalHysteresisState,
};

pub use cgroup::{
    get_cgroup_diagnostics, is_memory_reclaim_supported, migrate_to_background_cgroup,
    CgroupDiagnostics, CgroupMigrationSummary, CgroupVersion,
};
pub use daemon_state::{DaemonContext, DaemonRuntimeState, DaemonState};
pub use event_loop::{run_epoll_loop, run_fallback_loop, LoopAction, LoopEvent, LoopState};
#[cfg(unix)]
pub use event_loop::instant_to_itimerspec;
pub use freezer::{
    enumerate_frozen_uids, get_freezer_diagnostics, get_pid_freezer_state, get_uid_freezer_state,
    is_cached_apps_freezer_enabled, is_freezer_supported, FreezerDiagnostics, FreezerState,
};
pub use pidfile::{clean_stale_pid_files, get_running_pid, is_process_alive, PidLock};
pub use proc_metrics::{get_process_metrics, get_process_metrics_for_pid, ProcessMetrics};
pub use psi::{
    get_psi_diagnostics, is_psi_supported, read_io_pressure, read_memory_pressure, PsiDiagnostics,
    PsiMetrics, PsiPressureLevel, PsiWatcher,
};
pub use signals::{SignalEvent, SignalWatcher};
pub use watcher::DaemonRunner;
