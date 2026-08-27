use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::config::DaemonConfig;
use crate::engine::cancellation::CancellationToken;
use crate::engine::CleanEngine;
use crate::hardware::{
    get_charger_state, get_screen_state, read_thermal, ChargerState, ScreenState,
};
use crate::ipc::protocol::{CleanParams, Command, DaemonStatus, Response, ResponseData};

/// Formal state machine representation of the cleaner daemon lifecycle
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DaemonState {
    Starting,
    Idle,
    EvaluatingTriggers,
    CleaningScheduled,
    CleaningManual,
    PressureReclaiming(String),
    Preempted(String),
    ShuttingDown,
}

impl std::fmt::Display for DaemonState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DaemonState::Starting => write!(f, "Starting up"),
            DaemonState::Idle => write!(f, "Idle / Sleeping"),
            DaemonState::EvaluatingTriggers => write!(f, "Evaluating Triggers"),
            DaemonState::CleaningScheduled => write!(f, "Cleaning in progress (Scheduled)"),
            DaemonState::CleaningManual => write!(f, "Cleaning in progress (Manual Trigger)"),
            DaemonState::PressureReclaiming(lvl) => write!(f, "Memory Reclaim in progress ({lvl})"),
            DaemonState::Preempted(reason) => write!(f, "Preempted ({reason})"),
            DaemonState::ShuttingDown => write!(f, "Shutting down"),
        }
    }
}

/// Unified runtime state container for the cleaner daemon
#[derive(Debug, Clone)]
pub struct DaemonRuntimeState {
    pub state: DaemonState,
    pub last_cleaned_ts: Option<u64>,
    pub last_freed_bytes: u64,
    pub total_freed_bytes: u64,
    pub is_cleaning: bool,
    pub shutdown_requested: bool,
}

impl Default for DaemonRuntimeState {
    fn default() -> Self {
        Self {
            state: DaemonState::Idle,
            last_cleaned_ts: None,
            last_freed_bytes: 0,
            total_freed_bytes: 0,
            is_cleaning: false,
            shutdown_requested: false,
        }
    }
}

/// Shared daemon context providing coordinated access to configuration, engine, and runtime state
#[derive(Clone)]
pub struct DaemonContext {
    pub active_config_path: Arc<RwLock<Option<PathBuf>>>,
    pub config: Arc<RwLock<DaemonConfig>>,
    pub clean_engine: Arc<Mutex<CleanEngine>>,
    pub cancel_token: CancellationToken,
    pub runtime: Arc<RwLock<DaemonRuntimeState>>,
    pub start_time: Instant,
}

impl DaemonContext {
    pub fn new(config: DaemonConfig, active_config_path: Option<PathBuf>) -> Self {
        let clean_engine = Arc::new(Mutex::new(CleanEngine::new(config.clone())));
        let cancel_token = CancellationToken::new();

        Self {
            active_config_path: Arc::new(RwLock::new(active_config_path)),
            config: Arc::new(RwLock::new(config)),
            clean_engine,
            cancel_token,
            runtime: Arc::new(RwLock::new(DaemonRuntimeState::default())),
            start_time: Instant::now(),
        }
    }

    /// Read the current state enum
    pub fn get_state(&self) -> DaemonState {
        self.runtime
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .state
            .clone()
    }

    /// Update the current state enum
    pub fn set_state(&self, state: DaemonState) {
        let mut rt = self.runtime.write().unwrap_or_else(std::sync::PoisonError::into_inner);
        rt.state = state;
    }

    /// Returns whether a shutdown has been requested
    pub fn is_shutdown_requested(&self) -> bool {
        self.runtime
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .shutdown_requested
    }

    /// Returns whether a cleaning operation is currently executing
    pub fn is_cleaning(&self) -> bool {
        self.runtime
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_cleaning
    }

    /// Triggers immediate shutdown sequence
    pub fn trigger_shutdown(&self) {
        let mut rt = self.runtime.write().unwrap_or_else(std::sync::PoisonError::into_inner);
        rt.shutdown_requested = true;
        rt.state = DaemonState::ShuttingDown;
        self.cancel_token.cancel();
    }

    /// Performs cleanup of background workers and filesystem sockets during shutdown
    pub fn perform_shutdown_cleanup(&self) {
        self.trigger_shutdown();

        // Wait up to 2 seconds for worker thread to exit cleanly
        let wait_start = Instant::now();
        while self.is_cleaning() && wait_start.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(50));
        }

        // Clean up socket file if on filesystem
        let sock_path = self
            .config
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .socket_path
            .clone();
        if !sock_path.is_empty() {
            let _ = std::fs::remove_file(&sock_path);
        }

        log::info!("Graceful shutdown cleanup completed.");
    }

    /// Reloads daemon configuration strictly from disk
    pub fn reload_config(&self) -> Result<String, String> {
        let active_path = self
            .active_config_path
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let path_to_reload = active_path.as_deref();
        match DaemonConfig::reload_from_path(path_to_reload) {
            Ok(new_cfg) => {
                self.clean_engine
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .update_config(new_cfg.clone());
                *self.config.write().unwrap_or_else(std::sync::PoisonError::into_inner) = new_cfg;
                let msg = format!(
                    "Configuration reloaded successfully{}",
                    path_to_reload
                        .map(|p| format!(" from {}", p.display()))
                        .unwrap_or_default()
                );
                log::info!("{}", msg);
                Ok(msg)
            }
            Err(e) => {
                let err_msg = format!(
                    "Failed to reload configuration: {}. Keeping current config.",
                    e
                );
                log::warn!("{}", err_msg);
                Err(err_msg)
            }
        }
    }

    /// Evaluates whether environmental conditions (screen, battery, thermals) permit automatic cleaning
    pub fn evaluate_triggers(&self, screen_off_since: Option<Instant>) -> bool {
        let cfg = self
            .config
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();

        if cfg.require_screen_off {
            match screen_off_since {
                Some(since) => {
                    if since.elapsed() < Duration::from_secs(cfg.min_screen_off_secs) {
                        return false;
                    }
                }
                None => return false,
            }
        }

        if cfg.require_charging_for_deep_clean {
            let charger = get_charger_state();
            if charger != ChargerState::Charging && charger != ChargerState::Full {
                return false;
            }
        }

        let thermal = read_thermal();
        if thermal.max_soc_temp_c > cfg.max_soc_temp_c
            || thermal.battery_temp_c > cfg.max_battery_temp_c
        {
            log::info!(
                "Maintenance postponed due to high temperature (SoC: {:.1}C, Battery: {:.1}C)",
                thermal.max_soc_temp_c,
                thermal.battery_temp_c
            );
            return false;
        }

        true
    }

    /// Spawns a scheduled background maintenance worker thread
    pub fn spawn_maintenance_worker(&self) {
        {
            let mut rt = self.runtime.write().unwrap_or_else(std::sync::PoisonError::into_inner);
            if rt.shutdown_requested || rt.is_cleaning {
                return;
            }
            rt.is_cleaning = true;
            rt.state = DaemonState::CleaningScheduled;
        }

        self.cancel_token.reset();

        let clean_engine = self.clean_engine.clone();
        let cancel_token = self.cancel_token.clone();
        let runtime = self.runtime.clone();
        let cfg = self
            .config
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();

        let _ = std::thread::Builder::new()
            .name("cleaner-worker".to_string())
            .stack_size(256 * 1024)
            .spawn(move || {
                let params = CleanParams {
                    deep: true,
                    trim: cfg.optimization.fstrim_partitions,
                    zram_compact: cfg.optimization.zram_compaction,
                    dry_run: false,
                };

                log::info!("Starting background automatic maintenance cycle...");
                let report = {
                    let engine = clean_engine.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    engine.execute(&params, &cancel_token)
                };

                let now_ts = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();

                {
                    let mut rt = runtime.write().unwrap_or_else(std::sync::PoisonError::into_inner);
                    rt.last_cleaned_ts = Some(now_ts);
                    rt.last_freed_bytes = report.total_freed_bytes;
                    rt.total_freed_bytes += report.total_freed_bytes;
                    rt.state = DaemonState::Idle;
                    rt.is_cleaning = false;
                }

                // Immediately release all heap memory back to kernel
                crate::util::trim_heap_memory();

                log::info!(
                    "Automatic maintenance finished: {} freed across {} files in {} ms",
                    report.total_freed_bytes,
                    report.deleted_files_count,
                    report.duration_ms
                );
            });
    }

    /// Handles an incoming IPC command and returns a structured response
    pub fn handle_command(&self, cmd: Command) -> Response {
        if self.is_shutdown_requested() {
            return Response::Error("Daemon is currently shutting down".to_string());
        }

        match cmd {
            Command::Ping => Response::Success(ResponseData::Pong {
                version: env!("CARGO_PKG_VERSION").to_string(),
                uptime_secs: self.start_time.elapsed().as_secs(),
            }),
            Command::GetStatus => {
                let thermal = read_thermal();
                let is_charging = matches!(
                    get_charger_state(),
                    ChargerState::Charging | ChargerState::Full
                );
                let screen_str = match get_screen_state() {
                    ScreenState::On => "On",
                    ScreenState::Off => "Off",
                    ScreenState::Unknown => "Unknown",
                };

                let metrics = crate::system::proc_metrics::get_process_metrics();
                let rt = self.runtime.read().unwrap_or_else(std::sync::PoisonError::into_inner);

                let cpu_psi = crate::system::psi::read_cpu_pressure().map(|p| p.some.avg10);
                let io_psi = crate::system::psi::read_io_pressure().map(|p| p.some.avg10);
                let mem_psi = crate::system::psi::read_memory_pressure().map(|p| p.some.avg10);

                let battery_pct = crate::hardware::get_battery_percent().unwrap_or(50);
                let is_screen_on = matches!(get_screen_state(), ScreenState::On);

                let ctx = crate::system::idle::IdleContext {
                    screen: get_screen_state(),
                    screen_off_duration: None,
                    charging: is_charging,
                    battery_percent: battery_pct,
                    cpu_psi_pct: cpu_psi.map(crate::system::idle::SensorReading::available).unwrap_or_else(crate::system::idle::SensorReading::unsupported),
                    io_psi_pct: io_psi.map(crate::system::idle::SensorReading::available).unwrap_or_else(crate::system::idle::SensorReading::unsupported),
                    mem_psi_pct: mem_psi.map(crate::system::idle::SensorReading::available).unwrap_or_else(crate::system::idle::SensorReading::unsupported),
                    thermal_celsius: if thermal.max_soc_temp_c > 0.0 {
                        crate::system::idle::SensorReading::available(thermal.max_soc_temp_c)
                    } else {
                        crate::system::idle::SensorReading::unavailable()
                    },
                    thermal_source: Some("soc".to_string()),
                    stationary: !is_screen_on,
                    user_active: is_screen_on,
                };

                let assessment = crate::system::idle::IdlePolicy::evaluate(
                    &ctx,
                    crate::system::idle::IdleState::Active,
                    crate::system::idle::ThermalHysteresisState::Normal,
                    Duration::from_secs(300),
                );

                let status = DaemonStatus {
                    state: rt.state.to_string(),
                    uptime_secs: self.start_time.elapsed().as_secs(),
                    last_cleaned_ts: rt.last_cleaned_ts,
                    last_freed_bytes: rt.last_freed_bytes,
                    total_freed_bytes: rt.total_freed_bytes,
                    is_charging,
                    screen_state: screen_str.to_string(),
                    soc_temp_c: thermal.max_soc_temp_c,
                    battery_temp_c: thermal.battery_temp_c,
                    cpu_usage_pct: metrics.cpu_usage_pct,
                    ram_vm_size_bytes: metrics.vm_size_bytes,
                    ram_rss_bytes: metrics.rss_bytes,
                    ram_pss_bytes: metrics.pss_bytes,
                    idle_state: assessment.state.to_string(),
                    idle_score: assessment.score,
                    blockers: assessment.blockers.iter().map(|b| b.description().to_string()).collect(),
                };
                Response::Success(ResponseData::Status(status))
            }
            Command::GetIdleAssessment => {
                let screen = get_screen_state();
                let charger = get_charger_state();
                let is_charging = matches!(charger, ChargerState::Charging | ChargerState::Full);
                let thermal = read_thermal();
                let cpu_psi = crate::system::psi::read_cpu_pressure().map(|p| p.some.avg10);
                let io_psi = crate::system::psi::read_io_pressure().map(|p| p.some.avg10);
                let mem_psi = crate::system::psi::read_memory_pressure().map(|p| p.some.avg10);
                let battery_pct = crate::hardware::get_battery_percent().unwrap_or(50);
                let is_screen_on = matches!(screen, ScreenState::On);

                let ctx = crate::system::idle::IdleContext {
                    screen,
                    screen_off_duration: None,
                    charging: is_charging,
                    battery_percent: battery_pct,
                    cpu_psi_pct: cpu_psi.map(crate::system::idle::SensorReading::available).unwrap_or_else(crate::system::idle::SensorReading::unsupported),
                    io_psi_pct: io_psi.map(crate::system::idle::SensorReading::available).unwrap_or_else(crate::system::idle::SensorReading::unsupported),
                    mem_psi_pct: mem_psi.map(crate::system::idle::SensorReading::available).unwrap_or_else(crate::system::idle::SensorReading::unsupported),
                    thermal_celsius: if thermal.max_soc_temp_c > 0.0 {
                        crate::system::idle::SensorReading::available(thermal.max_soc_temp_c)
                    } else {
                        crate::system::idle::SensorReading::unavailable()
                    },
                    thermal_source: Some("soc".to_string()),
                    stationary: !is_screen_on,
                    user_active: is_screen_on,
                };

                let assessment = crate::system::idle::IdlePolicy::evaluate(
                    &ctx,
                    crate::system::idle::IdleState::Active,
                    crate::system::idle::ThermalHysteresisState::Normal,
                    Duration::from_secs(300),
                );

                Response::Success(ResponseData::Idle(assessment))
            }
            Command::TriggerClean(params) => {
                {
                    let mut rt = self.runtime.write().unwrap_or_else(std::sync::PoisonError::into_inner);
                    if rt.is_cleaning {
                        return Response::Error(
                            "Cleaning operation is already in progress".to_string(),
                        );
                    }
                    rt.is_cleaning = true;
                    rt.state = DaemonState::CleaningManual;
                }

                self.cancel_token.reset();

                log::info!(
                    "Executing manual clean job via IPC (deep: {}, trim: {}, zram: {})...",
                    params.deep,
                    params.trim,
                    params.zram_compact
                );

                let report = {
                    let engine = self.clean_engine.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    engine.execute(&params, &self.cancel_token)
                };

                let now_ts = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();

                {
                    let mut rt = self.runtime.write().unwrap_or_else(std::sync::PoisonError::into_inner);
                    rt.last_cleaned_ts = Some(now_ts);
                    rt.last_freed_bytes = report.total_freed_bytes;
                    rt.total_freed_bytes += report.total_freed_bytes;
                    rt.state = DaemonState::Idle;
                    rt.is_cleaning = false;
                }

                // Immediately release all heap memory back to kernel
                crate::util::trim_heap_memory();

                log::info!(
                    "Manual clean completed via IPC. Freed: {} across {} files",
                    report.total_freed_bytes,
                    report.deleted_files_count
                );
                Response::Success(ResponseData::Report(report))
            }
            Command::GetStats => {
                let metrics = crate::system::proc_metrics::get_process_metrics();
                let rt = self.runtime.read().unwrap_or_else(std::sync::PoisonError::into_inner);

                let status = DaemonStatus {
                    state: rt.state.to_string(),
                    uptime_secs: self.start_time.elapsed().as_secs(),
                    last_cleaned_ts: rt.last_cleaned_ts,
                    last_freed_bytes: rt.last_freed_bytes,
                    total_freed_bytes: rt.total_freed_bytes,
                    is_charging: false,
                    screen_state: "N/A".to_string(),
                    soc_temp_c: 0.0,
                    battery_temp_c: 0.0,
                    cpu_usage_pct: metrics.cpu_usage_pct,
                    ram_vm_size_bytes: metrics.vm_size_bytes,
                    ram_rss_bytes: metrics.rss_bytes,
                    ram_pss_bytes: metrics.pss_bytes,
                    idle_state: "N/A".to_string(),
                    idle_score: 0,
                    blockers: Vec::new(),
                };
                Response::Success(ResponseData::Status(status))
            }
            Command::Cancel => {
                self.cancel_token.cancel();
                self.set_state(DaemonState::Preempted("Cancelled by user".to_string()));
                Response::Success(ResponseData::Message(
                    "Clean operation cancelled".to_string(),
                ))
            }
            Command::ReloadConfig => match self.reload_config() {
                Ok(msg) => Response::Success(ResponseData::Message(msg)),
                Err(err) => Response::Error(err),
            },
            Command::StopDaemon => {
                log::info!("StopDaemon IPC command received. Triggering immediate shutdown...");
                self.trigger_shutdown();
                Response::Success(ResponseData::Message(
                    "Daemon stopping gracefully".to_string(),
                ))
            }
        }
    }
}
