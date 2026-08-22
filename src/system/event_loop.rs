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
    let mut ipc_raw_fds = Vec::new();
    if let Some(server) = ipc_server {
        for &fd in &server.get_raw_fds() {
            let mut ev = libc::epoll_event {
                events: libc::EPOLLIN as u32,
                u64: fd as u64,
            };
            unsafe {
                libc::epoll_ctl(epoll_fd, libc::EPOLL_CTL_ADD, fd, &mut ev);
            }
            ipc_raw_fds.push(fd);
        }
    }

    // 4. Initialize Linux Kernel PSI (Pressure Stall Information) Watcher
    let mut psi_watcher = if ctx
        .config
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .optimization
        .psi_adaptive_monitoring
    {
        let (moderate_ms, critical_ms) = {
            let cfg = ctx.config.read().unwrap_or_else(std::sync::PoisonError::into_inner);
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
    let mut screen_off_since: Option<Instant> = match get_screen_state() {
        ScreenState::Off => Some(Instant::now()),
        _ => None,
    };
    let has_uevent_socket = uevent_socket.is_some();

    let ctx_clone = ctx.clone();
    let ipc_handler = Arc::new(move |cmd: Command| ctx_clone.handle_command(cmd));

    log::info!("Daemon main epoll event loop initialized successfully.");

    while !ctx.is_shutdown_requested() {
        let timeout_ms = calculate_epoll_timeout(
            ctx,
            last_maintenance,
            screen_off_since,
            has_uevent_socket,
        );

        let nfds = unsafe {
            libc::epoll_wait(
                epoll_fd,
                events.as_mut_ptr(),
                events.len() as libc::c_int,
                timeout_ms,
            )
        };

        // Process active epoll events
        if nfds > 0 {
            let mut uevent_triggered = false;
            let mut signal_triggered = false;
            let mut ipc_triggered = false;
            let mut psi_level: Option<crate::system::psi::PsiPressureLevel> = None;

            for event in events.iter().take(nfds as usize) {
                let fd = event.u64 as i32;

                if Some(fd) == uevent_raw_fd {
                    uevent_triggered = true;
                } else if Some(fd) == signal_raw_fd {
                    signal_triggered = true;
                } else if ipc_raw_fds.contains(&fd) {
                    ipc_triggered = true;
                } else if let Some(ref pw) = psi_watcher {
                    if let Some(level) = pw.identify_fd(fd) {
                        psi_level = Some(level);
                    }
                }
            }

            // Handle IPC connections if IPC FD triggered
            if ipc_triggered {
                if let Some(server) = ipc_server {
                    server.accept_and_handle(ipc_handler.clone());
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
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
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

            // Handle Kernel Uevents (Backlight, Display, Power)
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
                            || ev.subsystem == "power_supply"
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
        }

        if ctx.is_shutdown_requested() {
            break;
        }

        // Only in fallback mode without uevent socket do we poll get_screen_state()
        if !has_uevent_socket {
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
        }

        // Evaluate maintenance schedule
        let interval_secs = ctx
            .config
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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

/// Calculates the optimal epoll_wait timeout in milliseconds to enable deep sleep / zero-idle overhead
#[cfg(unix)]
fn calculate_epoll_timeout(
    ctx: &DaemonContext,
    last_maintenance: Instant,
    screen_off_since: Option<Instant>,
    has_uevent_socket: bool,
) -> i32 {
    let cfg = ctx.config.read().unwrap_or_else(std::sync::PoisonError::into_inner);
    let interval = Duration::from_secs(cfg.maintenance_interval_secs);
    let elapsed = last_maintenance.elapsed();

    let mut timeout = if elapsed >= interval {
        // Maintenance is due, but if conditions (e.g. charging or thermals) aren't satisfied yet,
        // retry after 30 seconds to avoid busy-looping
        Duration::from_secs(30)
    } else {
        interval - elapsed
    };

    // If screen-off is required and screen is currently off, check if we need to wake up
    // when min_screen_off_secs is reached
    if cfg.require_screen_off {
        if let Some(off_since) = screen_off_since {
            let min_off = Duration::from_secs(cfg.min_screen_off_secs);
            let off_elapsed = off_since.elapsed();
            if off_elapsed < min_off {
                let remaining_off = min_off - off_elapsed;
                if elapsed >= interval {
                    // Maintenance already due; wake up right when screen-off duration requirement is satisfied
                    timeout = remaining_off;
                }
            }
        }
    }

    // Fallback: If uevent socket is unavailable, poll conservatively every 5 seconds
    if !has_uevent_socket {
        timeout = timeout.min(Duration::from_secs(5));
    }

    let millis = timeout.as_millis();
    if millis > (i32::MAX as u128) {
        i32::MAX
    } else {
        (millis as i32).max(100)
    }
}

/// Fallback event loop for platforms or configurations without Epoll
pub fn run_fallback_loop(ctx: &DaemonContext, ipc_server: Option<&IpcServer>) {
    let mut last_maintenance = Instant::now();
    let mut screen_off_since: Option<Instant> = match get_screen_state() {
        ScreenState::Off => Some(Instant::now()),
        _ => None,
    };

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
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .maintenance_interval_secs;
        let interval = Duration::from_secs(interval_secs);
        if last_maintenance.elapsed() >= interval {
            let should_run = ctx.evaluate_triggers(screen_off_since);
            if should_run && !ctx.is_cleaning() && !ctx.is_shutdown_requested() {
                ctx.spawn_maintenance_worker();
                last_maintenance = Instant::now();
            }
        }

        std::thread::sleep(Duration::from_millis(1000));
    }
}
