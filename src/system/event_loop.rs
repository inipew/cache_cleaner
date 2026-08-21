use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::hardware::{get_screen_state, ScreenState};
use crate::ipc::protocol::Command;
use crate::ipc::server::IpcServer;
use crate::system::daemon_state::{DaemonContext, DaemonState};

#[cfg(unix)]
use crate::system::signals::{SignalEvent, SignalWatcher};

/// Main Linux Epoll Reactor Event Loop
#[cfg(unix)]
pub fn run_epoll_loop(ctx: &DaemonContext, ipc_server: Option<&IpcServer>) {
    let epoll_fd = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
    if epoll_fd < 0 {
        log::warn!("Failed to create epoll instance, falling back to basic loop");
        run_fallback_loop(ctx, ipc_server);
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
            log::info!(
                "Kernel NETLINK_KOBJECT_UEVENT listener initialized (FD: {})",
                sock.fd
            );
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
            log::warn!(
                "Failed to open Netlink uevent socket: {}. Operating in polling fallback mode.",
                e
            );
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

    // 4. Initialize Linux Kernel PSI (Pressure Stall Information) Watcher
    let mut psi_watcher = if ctx
        .config
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .optimization
        .psi_adaptive_monitoring
    {
        let (moderate_ms, critical_ms) = {
            let cfg = ctx.config.read().unwrap_or_else(|p| p.into_inner());
            (
                cfg.optimization.psi_moderate_stall_ms,
                cfg.optimization.psi_critical_stall_ms,
            )
        };
        let pw = crate::system::psi::PsiWatcher::create(moderate_ms, critical_ms);
        for fd in pw.get_raw_fds() {
            let mut ev = libc::epoll_event {
                events: (libc::EPOLLPRI | libc::EPOLLERR) as u32,
                u64: fd as u64,
            };
            unsafe {
                libc::epoll_ctl(epoll_fd, libc::EPOLL_CTL_ADD, fd, &mut ev);
            }
        }
        Some(pw)
    } else {
        None
    };

    let mut events: Vec<libc::epoll_event> = vec![unsafe { std::mem::zeroed() }; 16];
    let mut last_maintenance = Instant::now();
    let mut screen_off_since: Option<Instant> = None;

    let ctx_clone = ctx.clone();
    let ipc_handler = Arc::new(move |cmd: Command| ctx_clone.handle_command(cmd));

    log::info!("Daemon main epoll event loop initialized successfully.");

    while !ctx.is_shutdown_requested() {
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
            let mut psi_level: Option<crate::system::psi::PsiPressureLevel> = None;

            for event in events.iter().take(nfds as usize) {
                let fd = event.u64 as i32;

                if Some(fd) == uevent_raw_fd {
                    uevent_triggered = true;
                } else if Some(fd) == signal_raw_fd {
                    signal_triggered = true;
                } else if let Some(ref pw) = psi_watcher {
                    if let Some(level) = pw.identify_fd(fd) {
                        psi_level = Some(level);
                    }
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
                                ctx.trigger_shutdown();
                                break;
                            }
                            SignalEvent::Reload => {
                                log::info!(
                                    "SIGHUP received via SignalFD. Reloading configuration..."
                                );
                                let _ = ctx.reload_config();
                            }
                            SignalEvent::Other(_) => {}
                        }
                    }
                }
            }

            if ctx.is_shutdown_requested() {
                break;
            }

            // Handle Kernel PSI Memory Pressure Events (Adaptive Controller)
            if let Some(level) = psi_level {
                if let Some(ref mut pw) = psi_watcher {
                    let cooldown = ctx
                        .config
                        .read()
                        .unwrap_or_else(|p| p.into_inner())
                        .optimization
                        .psi_cooldown_secs;
                    if pw.can_respond(cooldown) && !ctx.is_cleaning() {
                        pw.record_response();
                        ctx.set_state(DaemonState::PressureReclaiming(format!("{level}")));
                        log::warn!("Kernel PSI Memory Stall Trigger fired (Level: {})", level);

                        match level {
                            crate::system::psi::PsiPressureLevel::Moderate => {
                                log::info!("Executing Adaptive Soft Reclaim (64 MB) due to moderate PSI stall...");
                                let _ =
                                    crate::engine::memory::MemoryOptimizer::reclaim_cgroup_memory(
                                        64,
                                    );
                            }
                            crate::system::psi::PsiPressureLevel::Critical => {
                                log::warn!("Executing Deep Reclaim & ZRAM Compaction (128 MB) due to critical PSI stall...");
                                let _ =
                                    crate::engine::memory::MemoryOptimizer::reclaim_cgroup_memory(
                                        128,
                                    );
                                let _ = crate::engine::memory::MemoryOptimizer::compact_zram();
                                let _ = crate::engine::memory::MemoryOptimizer::compact_memory();
                            }
                            _ => {}
                        }

                        if !ctx.is_cleaning() {
                            ctx.set_state(DaemonState::Idle);
                        }
                    }
                }
            }

            // Handle Kernel Uevents
            if uevent_triggered {
                if let Some(ref sock) = uevent_socket {
                    let uevents = sock.read_events();
                    for ev in uevents {
                        log::debug!(
                            "Kernel Uevent received: subsystem={} action={} devpath={}",
                            ev.subsystem,
                            ev.action,
                            ev.devpath
                        );

                        if ev.subsystem == "backlight"
                            || ev.subsystem == "leds"
                            || ev.subsystem == "graphics"
                            || ev.subsystem == "drm"
                        {
                            let current_screen = get_screen_state();
                            if current_screen == ScreenState::On {
                                screen_off_since = None;
                                if ctx.is_cleaning() {
                                    log::info!("Screen turned ON (uevent: {}): Preempting ongoing cache clean operation!", ev.subsystem);
                                    ctx.cancel_token.cancel();
                                    ctx.set_state(DaemonState::Preempted(format!(
                                        "Screen turned ON ({})",
                                        ev.subsystem
                                    )));
                                }
                            } else if current_screen == ScreenState::Off
                                && screen_off_since.is_none()
                            {
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

        if ctx.is_shutdown_requested() {
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
                if ctx.is_cleaning() {
                    log::info!("Screen turned ON: Preempting ongoing cache clean operation!");
                    ctx.cancel_token.cancel();
                    ctx.set_state(DaemonState::Preempted("Screen turned ON".to_string()));
                }
            }
            ScreenState::Unknown => {}
        }

        // Evaluate maintenance schedule
        let interval_secs = ctx
            .config
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .maintenance_interval_secs;
        let interval = Duration::from_secs(interval_secs);
        if last_maintenance.elapsed() >= interval {
            let should_run = ctx.evaluate_triggers(screen_off_since);
            if should_run && !ctx.is_cleaning() && !ctx.is_shutdown_requested() {
                ctx.spawn_maintenance_worker();
                last_maintenance = Instant::now();
            }
        }
    }

    unsafe { libc::close(epoll_fd) };
}

/// Fallback event loop for platforms or configurations without Epoll
pub fn run_fallback_loop(ctx: &DaemonContext, ipc_server: Option<&IpcServer>) {
    let mut last_maintenance = Instant::now();
    let mut screen_off_since: Option<Instant> = None;

    let ctx_clone = ctx.clone();
    let ipc_handler = Arc::new(move |cmd: Command| ctx_clone.handle_command(cmd));

    while !ctx.is_shutdown_requested() {
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
                if ctx.is_cleaning() {
                    ctx.cancel_token.cancel();
                    ctx.set_state(DaemonState::Preempted("Screen turned ON".to_string()));
                }
            }
            ScreenState::Unknown => {}
        }

        let interval_secs = ctx
            .config
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .maintenance_interval_secs;
        let interval = Duration::from_secs(interval_secs);
        if last_maintenance.elapsed() >= interval {
            let should_run = ctx.evaluate_triggers(screen_off_since);
            if should_run && !ctx.is_cleaning() && !ctx.is_shutdown_requested() {
                ctx.spawn_maintenance_worker();
                last_maintenance = Instant::now();
            }
        }

        std::thread::sleep(Duration::from_millis(500));
    }
}
