use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::config::DaemonConfig;
use crate::engine::cancellation::CancellationToken;
use crate::engine::CleanEngine;
use crate::hardware::{get_charger_state, get_screen_state, read_thermal, ChargerState, ScreenState};
use crate::ipc::protocol::{CleanParams, Command, DaemonStatus, Response, ResponseData};
use crate::ipc::server::IpcServer;
use crate::system::cgroup::migrate_to_background_cgroup;
use crate::system::governor::set_idle_priorities;
use crate::system::pidfile::PidLock;

#[cfg(unix)]
use crate::system::signals::{SignalEvent, SignalWatcher};

mod parking_lot_sim {
    use std::sync::Mutex as StdMutex;
    pub struct Mutex<T>(StdMutex<T>);
    impl<T> Mutex<T> {
        pub fn new(val: T) -> Self {
            Self(StdMutex::new(val))
        }
        pub fn lock(&self) -> std::sync::MutexGuard<'_, T> {
            self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
        }
    }
}

#[derive(Clone)]
pub struct DaemonContext {
    pub active_config_path: Arc<parking_lot_sim::Mutex<Option<PathBuf>>>,
    pub config: Arc<parking_lot_sim::Mutex<DaemonConfig>>,
    pub clean_engine: Arc<parking_lot_sim::Mutex<CleanEngine>>,
    pub cancel_token: CancellationToken,
    pub is_cleaning: Arc<AtomicBool>,
    pub running: Arc<AtomicBool>,
    pub shutdown_requested: Arc<AtomicBool>,
    pub state_desc: Arc<parking_lot_sim::Mutex<String>>,
    pub start_time: Instant,
    pub last_cleaned_ts: Arc<parking_lot_sim::Mutex<Option<u64>>>,
    pub last_freed_bytes: Arc<parking_lot_sim::Mutex<u64>>,
    pub total_freed_bytes: Arc<parking_lot_sim::Mutex<u64>>,
}

impl DaemonContext {
    pub fn new(config: DaemonConfig, active_config_path: Option<PathBuf>) -> Self {
        let clean_engine = Arc::new(parking_lot_sim::Mutex::new(CleanEngine::new(config.clone())));
        let cancel_token = CancellationToken::new();
        let running = Arc::new(AtomicBool::new(true));
        let shutdown_requested = Arc::new(AtomicBool::new(false));
        let is_cleaning = Arc::new(AtomicBool::new(false));

        Self {
            active_config_path: Arc::new(parking_lot_sim::Mutex::new(active_config_path)),
            config: Arc::new(parking_lot_sim::Mutex::new(config)),
            clean_engine,
            cancel_token,
            is_cleaning,
            running,
            shutdown_requested,
            state_desc: Arc::new(parking_lot_sim::Mutex::new("Idle / Sleeping".to_string())),
            start_time: Instant::now(),
            last_cleaned_ts: Arc::new(parking_lot_sim::Mutex::new(None)),
            last_freed_bytes: Arc::new(parking_lot_sim::Mutex::new(0)),
            total_freed_bytes: Arc::new(parking_lot_sim::Mutex::new(0)),
        }
    }

    pub fn trigger_shutdown(&self) {
        self.shutdown_requested.store(true, Ordering::SeqCst);
        self.running.store(false, Ordering::SeqCst);
        self.cancel_token.cancel();
        *self.state_desc.lock() = "Shutting down".to_string();
    }

    pub fn perform_shutdown_cleanup(&self) {
        self.trigger_shutdown();

        // Wait up to 2 seconds for any active worker thread to exit cleanly
        let wait_start = Instant::now();
        while self.is_cleaning.load(Ordering::Relaxed) && wait_start.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(50));
        }

        // Clean up socket file if on filesystem
        let sock_path = self.config.lock().socket_path.clone();
        if !sock_path.is_empty() {
            let _ = std::fs::remove_file(&sock_path);
        }

        log::info!("Graceful shutdown cleanup completed.");
    }

    pub fn reload_config(&self) -> Result<String, String> {
        let active_path = self.active_config_path.lock().clone();
        let path_to_reload = active_path.as_deref();
        match DaemonConfig::reload_from_path(path_to_reload) {
            Ok(new_cfg) => {
                self.clean_engine.lock().update_config(new_cfg.clone());
                *self.config.lock() = new_cfg;
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
                let err_msg = format!("Failed to reload configuration: {}. Keeping current config.", e);
                log::warn!("{}", err_msg);
                Err(err_msg)
            }
        }
    }

    pub fn evaluate_triggers(&self, screen_off_since: Option<Instant>) -> bool {
        let cfg = self.config.lock().clone();

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

    pub fn spawn_maintenance_worker(&self) {
        if self.shutdown_requested.load(Ordering::Relaxed) {
            return;
        }

        if self.is_cleaning.swap(true, Ordering::SeqCst) {
            log::debug!("Maintenance worker already active, skipping spawn");
            return;
        }

        *self.state_desc.lock() = "Cleaning in progress (Scheduled)".to_string();
        self.cancel_token.reset();

        let clean_engine = self.clean_engine.clone();
        let cancel_token = self.cancel_token.clone();
        let is_cleaning = self.is_cleaning.clone();
        let state_desc = self.state_desc.clone();
        let last_cleaned_ts = self.last_cleaned_ts.clone();
        let last_freed_bytes = self.last_freed_bytes.clone();
        let total_freed_bytes = self.total_freed_bytes.clone();
        let cfg = self.config.lock().clone();

        let _ = std::thread::Builder::new()
            .name("cleaner-worker".to_string())
            .spawn(move || {
                let params = CleanParams {
                    deep: true,
                    trim: cfg.optimization.fstrim_partitions,
                    zram_compact: cfg.optimization.zram_compaction,
                    dry_run: false,
                };

                log::info!("Starting background automatic maintenance cycle...");
                let report = {
                    let engine = clean_engine.lock();
                    engine.execute(&params, &cancel_token)
                };

                let now_ts = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();

                *last_cleaned_ts.lock() = Some(now_ts);
                *last_freed_bytes.lock() = report.total_freed_bytes;
                *total_freed_bytes.lock() += report.total_freed_bytes;
                *state_desc.lock() = "Idle / Sleeping".to_string();
                is_cleaning.store(false, Ordering::SeqCst);

                log::info!(
                    "Automatic maintenance finished: {} freed across {} files in {} ms",
                    report.total_freed_bytes,
                    report.deleted_files_count,
                    report.duration_ms
                );
            });
    }

    pub fn handle_command(&self, cmd: Command) -> Response {
        if self.shutdown_requested.load(Ordering::Relaxed) {
            return Response::Error("Daemon is currently shutting down".to_string());
        }

        match cmd {
            Command::Ping => Response::Success(ResponseData::Pong {
                version: env!("CARGO_PKG_VERSION").to_string(),
                uptime_secs: self.start_time.elapsed().as_secs(),
            }),
            Command::GetStatus => {
                let thermal = read_thermal();
                let is_charging = matches!(get_charger_state(), ChargerState::Charging | ChargerState::Full);
                let screen_str = match get_screen_state() {
                    ScreenState::On => "On",
                    ScreenState::Off => "Off",
                    ScreenState::Unknown => "Unknown",
                };

                let metrics = crate::system::proc_metrics::get_process_metrics();

                let status = DaemonStatus {
                    state: self.state_desc.lock().clone(),
                    uptime_secs: self.start_time.elapsed().as_secs(),
                    last_cleaned_ts: *self.last_cleaned_ts.lock(),
                    last_freed_bytes: *self.last_freed_bytes.lock(),
                    total_freed_bytes: *self.total_freed_bytes.lock(),
                    is_charging,
                    screen_state: screen_str.to_string(),
                    soc_temp_c: thermal.max_soc_temp_c,
                    battery_temp_c: thermal.battery_temp_c,
                    cpu_usage_pct: metrics.cpu_usage_pct,
                    ram_vm_size_bytes: metrics.vm_size_bytes,
                    ram_rss_bytes: metrics.rss_bytes,
                    ram_pss_bytes: metrics.pss_bytes,
                };
                Response::Success(ResponseData::Status(status))
            }
            Command::TriggerClean(params) => {
                if self.is_cleaning.swap(true, Ordering::SeqCst) {
                    return Response::Error("Cleaning operation is already in progress".to_string());
                }

                *self.state_desc.lock() = "Cleaning in progress (Manual Trigger)".to_string();
                self.cancel_token.reset();

                log::info!("Executing manual clean job via IPC (deep: {}, trim: {}, zram: {})...", params.deep, params.trim, params.zram_compact);

                let report = {
                    let engine = self.clean_engine.lock();
                    engine.execute(&params, &self.cancel_token)
                };

                let now_ts = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();

                *self.last_cleaned_ts.lock() = Some(now_ts);
                *self.last_freed_bytes.lock() = report.total_freed_bytes;
                *self.total_freed_bytes.lock() += report.total_freed_bytes;
                *self.state_desc.lock() = "Idle / Sleeping".to_string();
                self.is_cleaning.store(false, Ordering::SeqCst);

                log::info!("Manual clean completed via IPC. Freed: {} across {} files", report.total_freed_bytes, report.deleted_files_count);
                Response::Success(ResponseData::Report(report))
            }
            Command::GetStats => {
                let metrics = crate::system::proc_metrics::get_process_metrics();

                let status = DaemonStatus {
                    state: self.state_desc.lock().clone(),
                    uptime_secs: self.start_time.elapsed().as_secs(),
                    last_cleaned_ts: *self.last_cleaned_ts.lock(),
                    last_freed_bytes: *self.last_freed_bytes.lock(),
                    total_freed_bytes: *self.total_freed_bytes.lock(),
                    is_charging: false,
                    screen_state: "N/A".to_string(),
                    soc_temp_c: 0.0,
                    battery_temp_c: 0.0,
                    cpu_usage_pct: metrics.cpu_usage_pct,
                    ram_vm_size_bytes: metrics.vm_size_bytes,
                    ram_rss_bytes: metrics.rss_bytes,
                    ram_pss_bytes: metrics.pss_bytes,
                };
                Response::Success(ResponseData::Status(status))
            }
            Command::Cancel => {
                self.cancel_token.cancel();
                *self.state_desc.lock() = "Cancelled".to_string();
                Response::Success(ResponseData::Message("Clean operation cancelled".to_string()))
            }
            Command::ReloadConfig => match self.reload_config() {
                Ok(msg) => Response::Success(ResponseData::Message(msg)),
                Err(err) => Response::Error(err),
            },
            Command::StopDaemon => {
                log::info!("StopDaemon IPC command received. Triggering immediate shutdown...");
                self.trigger_shutdown();
                Response::Success(ResponseData::Message("Daemon stopping gracefully".to_string()))
            }
        }
    }
}

pub struct DaemonRunner {
    ctx: Arc<DaemonContext>,
}

impl DaemonRunner {
    pub fn new(config: DaemonConfig, active_config_path: Option<PathBuf>) -> Self {
        Self {
            ctx: Arc::new(DaemonContext::new(config, active_config_path)),
        }
    }

    pub fn run(&mut self) {
        log::info!("Starting Android Cache Cleaner Daemon...");

        #[cfg(unix)]
        unsafe {
            // Detach session and set safe umask
            libc::setsid();
            libc::umask(0o027);
        }

        // 1. Acquire exclusive PID file lock (Guarantees zero duplicate instances)
        let _pid_lock = match PidLock::acquire() {
            Ok(lock) => {
                log::info!("Daemon PID lock acquired at {}", lock.path().display());
                lock
            }
            Err(e) => {
                log::error!("Cannot start daemon: {}", e);
                eprintln!("[!] Cannot start daemon: {}", e);
                return;
            }
        };

        // 2. Set background Cgroup and CPU/IO low priorities
        migrate_to_background_cgroup();
        set_idle_priorities();

        // 3. Initialize IPC Server
        let socket_path = self.ctx.config.lock().socket_path.clone();
        let abstract_socket_name = self.ctx.config.lock().abstract_socket_name.clone();

        let ipc_server = match IpcServer::bind(&socket_path, &abstract_socket_name) {
            Ok(server) => Some(server),
            Err(e) => {
                log::warn!("Failed to initialize IPC server: {}. Continuing without IPC.", e);
                None
            }
        };

        // 4. Linux Kernel Epoll Event Loop
        #[cfg(unix)]
        self.run_epoll_loop(ipc_server.as_ref());

        #[cfg(not(unix))]
        self.run_fallback_loop(ipc_server.as_ref());

        // 5. Graceful Cleanup and Termination
        self.ctx.perform_shutdown_cleanup();
        log::info!("Cache Cleaner Daemon terminated completely and safely.");
    }

    #[cfg(unix)]
    fn run_epoll_loop(&mut self, ipc_server: Option<&IpcServer>) {
        let epoll_fd = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
        if epoll_fd < 0 {
            log::warn!("Failed to create epoll instance, falling back to basic loop");
            self.run_fallback_loop(ipc_server);
            return;
        }

        // 1. Initialize SignalFD watcher for SIGINT, SIGTERM, SIGHUP
        let signal_watcher = match SignalWatcher::create() {
            Ok(sw) => {
                let mut ev = libc::epoll_event {
                    events: libc::EPOLLIN as u32,
                    u64: sw.fd() as u64,
                };
                unsafe {
                    libc::epoll_ctl(epoll_fd, libc::EPOLL_CTL_ADD, sw.fd(), &mut ev);
                }
                Some(sw)
            }
            Err(e) => {
                log::warn!("Failed to create SignalFD watcher: {}", e);
                None
            }
        };
        let signal_raw_fd = signal_watcher.as_ref().map(|s| s.fd());

        // 2. Open and register Linux Kernel Netlink Uevent Socket
        let uevent_socket = match crate::hardware::UeventSocket::open() {
            Ok(sock) => {
                log::info!("Kernel NETLINK_KOBJECT_UEVENT listener initialized (FD: {})", sock.fd);
                let mut ev = libc::epoll_event {
                    events: libc::EPOLLIN as u32,
                    u64: sock.fd as u64,
                };
                unsafe {
                    libc::epoll_ctl(epoll_fd, libc::EPOLL_CTL_ADD, sock.fd, &mut ev);
                }
                Some(sock)
            }
            Err(e) => {
                log::warn!("Failed to open Netlink uevent socket: {}. Operating in polling fallback mode.", e);
                None
            }
        };
        let uevent_raw_fd = uevent_socket.as_ref().map(|s| s.fd);

        // 3. Register IPC listener FDs in epoll
        if let Some(server) = ipc_server {
            for &fd in &server.get_raw_fds() {
                let mut ev = libc::epoll_event {
                    events: libc::EPOLLIN as u32,
                    u64: fd as u64,
                };
                unsafe {
                    libc::epoll_ctl(epoll_fd, libc::EPOLL_CTL_ADD, fd, &mut ev);
                }
            }
        }

        let mut events: Vec<libc::epoll_event> = vec![unsafe { std::mem::zeroed() }; 16];
        let mut last_maintenance = Instant::now();
        let mut screen_off_since: Option<Instant> = None;

        let ctx_clone = self.ctx.clone();
        let ipc_handler = Arc::new(move |cmd: Command| -> Response {
            ctx_clone.handle_command(cmd)
        });

        log::info!("Daemon main epoll event loop initialized successfully.");

        while self.ctx.running.load(Ordering::Relaxed) && !self.ctx.shutdown_requested.load(Ordering::Relaxed) {
            // Handle any incoming IPC connections immediately
            if let Some(server) = ipc_server {
                server.accept_and_handle(ipc_handler.clone());
            }

            let nfds = unsafe {
                libc::epoll_wait(
                    epoll_fd,
                    events.as_mut_ptr(),
                    events.len() as libc::c_int,
                    500,
                )
            };

            // Process active epoll events
            if nfds > 0 {
                let mut uevent_triggered = false;
                let mut signal_triggered = false;

                for i in 0..nfds as usize {
                    let fd = events[i].u64 as i32;

                    if Some(fd) == uevent_raw_fd {
                        uevent_triggered = true;
                    } else if Some(fd) == signal_raw_fd {
                        signal_triggered = true;
                    }
                }

                // Handle Kernel Signals (SIGINT, SIGTERM, SIGHUP)
                if signal_triggered {
                    if let Some(ref sw) = signal_watcher {
                        let sig_events = sw.read_events();
                        for sig in sig_events {
                            match sig {
                                SignalEvent::Shutdown => {
                                    log::info!("Shutdown signal received via SignalFD. Initiating graceful stop...");
                                    self.ctx.trigger_shutdown();
                                    break;
                                }
                                SignalEvent::Reload => {
                                    log::info!("SIGHUP received via SignalFD. Reloading configuration...");
                                    let _ = self.ctx.reload_config();
                                }
                                SignalEvent::Other(_) => {}
                            }
                        }
                    }
                }

                if self.ctx.shutdown_requested.load(Ordering::Relaxed) {
                    break;
                }

                // Handle Kernel Uevents
                if uevent_triggered {
                    if let Some(ref sock) = uevent_socket {
                        let uevents = sock.read_events();
                        for ev in uevents {
                            log::debug!("Kernel Uevent received: subsystem={} action={} devpath={}", ev.subsystem, ev.action, ev.devpath);

                            if ev.subsystem == "backlight"
                                || ev.subsystem == "leds"
                                || ev.subsystem == "graphics"
                                || ev.subsystem == "drm"
                            {
                                let current_screen = get_screen_state();
                                if current_screen == ScreenState::On {
                                    screen_off_since = None;
                                    if self.ctx.is_cleaning.load(Ordering::Relaxed) {
                                        log::info!("Screen turned ON (uevent: {}): Preempting ongoing cache clean operation!", ev.subsystem);
                                        self.ctx.cancel_token.cancel();
                                    }
                                } else if current_screen == ScreenState::Off && screen_off_since.is_none() {
                                    screen_off_since = Some(Instant::now());
                                }
                            }
                        }
                    }
                }

                // Handle IPC connections
                if let Some(server) = ipc_server {
                    server.accept_and_handle(ipc_handler.clone());
                }
            }

            if self.ctx.shutdown_requested.load(Ordering::Relaxed) {
                break;
            }

            // Periodic screen state check & preemption guard
            let screen = get_screen_state();
            match screen {
                ScreenState::Off => {
                    if screen_off_since.is_none() {
                        screen_off_since = Some(Instant::now());
                    }
                }
                ScreenState::On => {
                    if screen_off_since.is_some() {
                        screen_off_since = None;
                    }
                    if self.ctx.is_cleaning.load(Ordering::Relaxed) {
                        log::info!("Screen turned ON: Preempting ongoing cache clean operation!");
                        self.ctx.cancel_token.cancel();
                    }
                }
                ScreenState::Unknown => {}
            }

            // Evaluate maintenance schedule
            let interval = Duration::from_secs(self.ctx.config.lock().maintenance_interval_secs);
            if last_maintenance.elapsed() >= interval {
                let should_run = self.ctx.evaluate_triggers(screen_off_since);
                if should_run && !self.ctx.is_cleaning.load(Ordering::Relaxed) && !self.ctx.shutdown_requested.load(Ordering::Relaxed) {
                    self.ctx.spawn_maintenance_worker();
                    last_maintenance = Instant::now();
                }
            }
        }

        unsafe { libc::close(epoll_fd) };
    }

    #[allow(dead_code)]
    fn run_fallback_loop(&mut self, ipc_server: Option<&IpcServer>) {
        let mut last_maintenance = Instant::now();
        let mut screen_off_since: Option<Instant> = None;

        let ctx_clone = self.ctx.clone();
        let ipc_handler = Arc::new(move |cmd: Command| -> Response {
            ctx_clone.handle_command(cmd)
        });

        while self.ctx.running.load(Ordering::Relaxed) && !self.ctx.shutdown_requested.load(Ordering::Relaxed) {
            if let Some(server) = ipc_server {
                #[cfg(unix)]
                server.accept_and_handle(ipc_handler.clone());
                #[cfg(not(unix))]
                let _ = server;
            }

            let screen = get_screen_state();
            match screen {
                ScreenState::Off => {
                    if screen_off_since.is_none() {
                        screen_off_since = Some(Instant::now());
                    }
                }
                ScreenState::On => {
                    screen_off_since = None;
                    if self.ctx.is_cleaning.load(Ordering::Relaxed) {
                        self.ctx.cancel_token.cancel();
                    }
                }
                ScreenState::Unknown => {}
            }

            let interval = Duration::from_secs(self.ctx.config.lock().maintenance_interval_secs);
            if last_maintenance.elapsed() >= interval {
                let should_run = self.ctx.evaluate_triggers(screen_off_since);
                if should_run && !self.ctx.is_cleaning.load(Ordering::Relaxed) && !self.ctx.shutdown_requested.load(Ordering::Relaxed) {
                    self.ctx.spawn_maintenance_worker();
                    last_maintenance = Instant::now();
                }
            }

            std::thread::sleep(Duration::from_millis(500));
        }
    }
}
