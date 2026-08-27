use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::hardware::{get_screen_state, ScreenState, UeventMessage};
use crate::ipc::protocol::Command;
use crate::ipc::server::IpcServer;
use crate::system::daemon_state::{DaemonContext, DaemonState};
use crate::system::psi::PsiPressureLevel;

#[cfg(unix)]
use crate::system::signals::{SignalEvent, SignalWatcher};

// ─────────────────────────────────────────────────────────────────────────────
// Pure Event Loop Reducer & State Machine
// ─────────────────────────────────────────────────────────────────────────────

/// Discrete input events for the event loop state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopEvent {
    /// The deadline timer expired on `timer_fd`.
    TimerExpired,
    /// A kernel Netlink uevent was received.
    Uevent(UeventMessage),
    /// Netlink socket buffer overflow (ENOBUFS) or socket read error.
    UeventBufferOverflow,
    /// OS signal received via SignalFD.
    Signal(SignalEvent),
    /// An IPC listener socket has a pending connection (EPOLLIN).
    IpcReadable,
    /// Kernel PSI memory pressure trigger fired.
    PsiPressure(PsiPressureLevel),
    /// Reconciled authoritative screen state from sysfs.
    ReconcileScreen(ScreenState),
}

/// Actions emitted by the pure state machine for the I/O driver to execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopAction {
    /// No side effect required.
    None,
    /// Trigger background maintenance cleaning cycle.
    RunMaintenance,
    /// Preempt ongoing cleaning due to screen turn-on.
    PreemptCleaning(String),
    /// Terminate event loop gracefully.
    Shutdown,
    /// Reload configuration.
    ReloadConfig,
    /// Accept and handle incoming IPC connections.
    HandleIpc,
    /// Reclaim memory due to PSI stall.
    ReclaimMemory(PsiPressureLevel),
    /// Perform authoritative sysfs query to reconcile screen state.
    ReconcileSysfs,
}

/// Self-contained state for the event loop's deadline scheduler.
///
/// # Invariants
///
/// 1. **Zero-Idle Overhead**: If no external event arrives and no deadline expires,
///    `epoll_wait(-1)` will not wake up the CPU.
/// 2. **CLOCK_MONOTONIC Semantics**: `timerfd` uses `CLOCK_MONOTONIC`. It does NOT
///    wake up the CPU from deep Android kernel suspend. Deadlines are evaluated
///    naturally when the CPU resumes.
/// 3. **Drift-Free Deadlines**: Successful maintenance advances `maintenance_deadline`
///    by `interval` from its previous value rather than resetting from `Instant::now()`.
/// 4. **Bounded Retry Backoff**: When triggers fail (thermals, charging), retry uses
///    exponential backoff (5s → 10s → 20s → 40s → 60s max) rather than constant busy-polling.
#[derive(Debug, Clone)]
pub struct LoopState {
    /// Authoritative screen state.
    pub screen_state: ScreenState,
    /// Instant when screen turned off, passed to `evaluate_triggers`.
    pub screen_off_since: Option<Instant>,
    /// Whether Netlink socket is healthy (false triggers sysfs reconciliation).
    pub uevent_ok: bool,
    /// Absolute monotonic deadline for the next maintenance cycle.
    pub maintenance_deadline: Instant,
    /// Absolute deadline when `min_screen_off_secs` is satisfied (None if screen is On).
    pub screen_off_deadline: Option<Instant>,
    /// Backoff retry deadline when triggers are not met.
    pub retry_deadline: Option<Instant>,
    /// Current retry backoff duration in seconds (5s to 60s).
    pub retry_backoff_secs: u64,
}

impl LoopState {
    pub const INITIAL_RETRY_BACKOFF_SECS: u64 = 5;
    pub const MAX_RETRY_BACKOFF_SECS: u64 = 60;

    #[must_use]
    pub fn new(maintenance_interval_secs: u64, now: Instant) -> Self {
        Self {
            screen_state: ScreenState::Unknown,
            screen_off_since: None,
            uevent_ok: true,
            maintenance_deadline: now + Duration::from_secs(maintenance_interval_secs),
            screen_off_deadline: None,
            retry_deadline: None,
            retry_backoff_secs: Self::INITIAL_RETRY_BACKOFF_SECS,
        }
    }

    /// Returns the earliest active monotonic deadline to arm `timerfd`.
    #[must_use]
    pub fn earliest_deadline(&self) -> Instant {
        let mut earliest = self.maintenance_deadline;
        if let Some(d) = self.screen_off_deadline {
            if d < earliest {
                earliest = d;
            }
        }
        if let Some(d) = self.retry_deadline {
            if d < earliest {
                earliest = d;
            }
        }
        earliest
    }

    /// Pure reducer function: processes an event and transitions state, returning actions.
    pub fn reduce(
        &mut self,
        event: LoopEvent,
        now: Instant,
        min_screen_off_secs: u64,
        maintenance_interval_secs: u64,
        is_cleaning: bool,
    ) -> Vec<LoopAction> {
        let mut actions = Vec::new();

        match event {
            LoopEvent::TimerExpired => {
                // 1. Check screen-off requirement deadline
                if let Some(off_dl) = self.screen_off_deadline {
                    if now >= off_dl {
                        self.screen_off_deadline = None;
                    }
                }

                // 2. Check if maintenance or retry deadline expired
                let maintenance_due = now >= self.maintenance_deadline;
                let retry_due = self.retry_deadline.is_some_and(|r| now >= r);

                if maintenance_due || retry_due {
                    actions.push(LoopAction::RunMaintenance);
                }
            }

            LoopEvent::Uevent(ev) => {
                let is_display_or_power = ev.subsystem == "backlight"
                    || ev.subsystem == "leds"
                    || ev.subsystem == "graphics"
                    || ev.subsystem == "drm"
                    || ev.subsystem == "power_supply";

                if is_display_or_power {
                    // Netlink event indicates display/power change: reconcile authoritatively
                    actions.push(LoopAction::ReconcileSysfs);
                }
            }

            LoopEvent::UeventBufferOverflow => {
                self.uevent_ok = false;
                actions.push(LoopAction::ReconcileSysfs);
            }

            LoopEvent::ReconcileScreen(new_state) => {
                self.uevent_ok = true;
                match new_state {
                    ScreenState::On => {
                        let was_off = self.screen_state != ScreenState::On;
                        self.screen_state = ScreenState::On;
                        self.screen_off_since = None;
                        self.screen_off_deadline = None;
                        self.reset_retry_backoff();

                        if was_off && is_cleaning {
                            actions.push(LoopAction::PreemptCleaning(
                                "Screen turned ON".to_string(),
                            ));
                        }
                    }
                    ScreenState::Off => {
                        if self.screen_state != ScreenState::Off {
                            self.screen_state = ScreenState::Off;
                            self.screen_off_since = Some(now);
                            self.screen_off_deadline =
                                Some(now + Duration::from_secs(min_screen_off_secs));
                            self.reset_retry_backoff();
                        }
                    }
                    ScreenState::Unknown => {}
                }
            }

            LoopEvent::Signal(SignalEvent::Shutdown) => {
                actions.push(LoopAction::Shutdown);
            }

            LoopEvent::Signal(SignalEvent::Reload) => {
                self.maintenance_deadline =
                    now + Duration::from_secs(maintenance_interval_secs);
                self.retry_deadline = None;
                self.reset_retry_backoff();
                actions.push(LoopAction::ReloadConfig);
            }

            LoopEvent::Signal(SignalEvent::Other(_)) => {}

            LoopEvent::IpcReadable => {
                actions.push(LoopAction::HandleIpc);
            }

            LoopEvent::PsiPressure(level) => {
                actions.push(LoopAction::ReclaimMemory(level));
            }
        }

        actions
    }

    /// Advances maintenance deadline after a successful cleaning cycle.
    pub fn on_maintenance_success(&mut self, now: Instant, interval_secs: u64) {
        let interval = Duration::from_secs(interval_secs);
        self.maintenance_deadline += interval;
        if self.maintenance_deadline < now {
            self.maintenance_deadline = now + interval;
        }
        self.retry_deadline = None;
        self.reset_retry_backoff();
    }

    /// Sets an exponential backoff retry deadline when triggers are not satisfied.
    pub fn on_maintenance_postponed(&mut self, now: Instant, interval_secs: u64) {
        let backoff = self.retry_backoff_secs;
        self.retry_deadline = Some(now + Duration::from_secs(backoff));
        // Exponential backoff: 5s -> 10s -> 20s -> 40s -> 60s max
        self.retry_backoff_secs = (backoff * 2).min(Self::MAX_RETRY_BACKOFF_SECS);
        // Ensure main maintenance deadline is not stuck in the past
        if self.maintenance_deadline <= now {
            self.maintenance_deadline = now + Duration::from_secs(interval_secs);
        }
    }

    /// Resets retry backoff to initial minimum.
    pub fn reset_retry_backoff(&mut self) {
        self.retry_backoff_secs = Self::INITIAL_RETRY_BACKOFF_SECS;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// timerfd helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Converts an absolute `Instant` monotonic deadline to a kernel `itimerspec`.
///
/// # Invariants
/// - Uses `CLOCK_MONOTONIC` domain consistent with `timerfd_create`.
/// - If `deadline <= now`, arm for 1 nanosecond (minimum positive non-zero delay).
/// - Safely handles nanosecond carry arithmetic and saturating addition.
#[cfg(unix)]
#[must_use]
pub fn instant_to_itimerspec(deadline: Instant, now: Instant) -> libc::itimerspec {
    let remaining = if deadline > now {
        deadline.saturating_duration_since(now)
    } else {
        Duration::from_nanos(1) // 0 disarms the timer, so 1ns guarantees immediate expiration
    };

    let mut mono_now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut mono_now) };

    let total_nsec = mono_now.tv_nsec as u64 + remaining.subsec_nanos() as u64;
    let carry_secs = (total_nsec / 1_000_000_000) as i64;
    let rem_nsec = (total_nsec % 1_000_000_000) as i64;

    let target_sec = mono_now
        .tv_sec
        .saturating_add(remaining.as_secs() as i64)
        .saturating_add(carry_secs);

    libc::itimerspec {
        it_interval: libc::timespec { tv_sec: 0, tv_nsec: 0 }, // one-shot
        it_value: libc::timespec {
            tv_sec: target_sec,
            tv_nsec: rem_nsec,
        },
    }
}

/// Arms the `timerfd` to fire at `deadline` using `TFD_TIMER_ABSTIME`.
#[cfg(unix)]
fn arm_timer(timer_fd: libc::c_int, deadline: Instant) {
    let itimer = instant_to_itimerspec(deadline, Instant::now());
    let ret = unsafe {
        libc::timerfd_settime(timer_fd, libc::TFD_TIMER_ABSTIME, &itimer, std::ptr::null_mut())
    };
    if ret < 0 {
        log::warn!("timerfd_settime failed: {}", std::io::Error::last_os_error());
    }
}

/// Reads and drains the 8-byte expiration counter from `timer_fd`.
///
/// **MUST** be called on `EPOLLIN` to clear level-triggered event and avoid 100% CPU busy-loop.
#[cfg(unix)]
fn drain_timer(timer_fd: libc::c_int) {
    let mut buf = [0u8; 8];
    let n = unsafe {
        libc::read(timer_fd, buf.as_mut_ptr().cast::<libc::c_void>(), 8)
    };
    if n < 0 {
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::EAGAIN) {
            log::debug!("timerfd drain error: {e}");
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Main Linux Epoll Reactor Event Loop
// ─────────────────────────────────────────────────────────────────────────────

/// Main Linux Epoll Reactor — timerfd-based, fully event-driven, zero idle CPU wake-ups.
#[cfg(unix)]
pub fn run_epoll_loop(ctx: &DaemonContext, ipc_server: Option<&IpcServer>) {
    let epoll_fd = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
    if epoll_fd < 0 {
        log::warn!("epoll_create1 failed, falling back to basic loop");
        run_fallback_loop(ctx, ipc_server);
        return;
    }

    let epoll_add = |fd: libc::c_int, events: u32| {
        let mut ev = libc::epoll_event { events, u64: fd as u64 };
        unsafe { libc::epoll_ctl(epoll_fd, libc::EPOLL_CTL_ADD, fd, &mut ev) };
    };

    // 1. SignalFD for SIGINT, SIGTERM, SIGHUP
    let signal_watcher = match SignalWatcher::create() {
        Ok(sw) => {
            epoll_add(sw.fd(), libc::EPOLLIN as u32);
            Some(sw)
        }
        Err(e) => {
            log::warn!("Failed to create SignalFD watcher: {e}");
            None
        }
    };
    let signal_raw_fd = signal_watcher.as_ref().map(SignalWatcher::fd);

    // 2. Netlink Uevent Socket (Subscribed first to prevent startup race)
    let uevent_socket = match crate::hardware::UeventSocket::open() {
        Ok(sock) => {
            log::info!(
                "Kernel NETLINK_KOBJECT_UEVENT listener initialized (FD: {})",
                sock.fd
            );
            epoll_add(sock.fd, libc::EPOLLIN as u32);
            Some(sock)
        }
        Err(e) => {
            log::warn!("Failed to open Netlink uevent socket: {e}");
            None
        }
    };
    let uevent_raw_fd = uevent_socket.as_ref().map(|s| s.fd);

    // 3. IPC Server listener FDs
    let mut ipc_raw_fds = Vec::new();
    if let Some(server) = ipc_server {
        for &fd in &server.get_raw_fds() {
            epoll_add(fd, libc::EPOLLIN as u32);
            ipc_raw_fds.push(fd);
        }
    }

    // 4. Kernel PSI Watcher
    let mut psi_watcher = {
        let cfg = ctx.config.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        if cfg.optimization.psi_adaptive_monitoring {
            let moderate_ms = cfg.optimization.psi_moderate_stall_ms;
            let critical_ms = cfg.optimization.psi_critical_stall_ms;
            let pw = crate::system::psi::PsiWatcher::create(moderate_ms, critical_ms);
            for fd in pw.get_raw_fds() {
                epoll_add(fd, (libc::EPOLLPRI | libc::EPOLLERR) as u32);
            }
            Some(pw)
        } else {
            None
        }
    };

    // 5. One-Shot CLOCK_MONOTONIC timerfd
    let timer_fd = unsafe {
        libc::timerfd_create(libc::CLOCK_MONOTONIC, libc::TFD_NONBLOCK | libc::TFD_CLOEXEC)
    };
    if timer_fd < 0 {
        log::warn!("timerfd_create failed, falling back to basic loop");
        unsafe { libc::close(epoll_fd) };
        run_fallback_loop(ctx, ipc_server);
        return;
    }
    epoll_add(timer_fd, libc::EPOLLIN as u32);

    let (maintenance_interval_secs, min_screen_off_secs) = {
        let cfg = ctx.config.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        (cfg.maintenance_interval_secs, cfg.min_screen_off_secs)
    };

    // 6. Initialize State Machine
    let mut state = LoopState::new(maintenance_interval_secs, Instant::now());

    // 7. Startup Synchronization:
    //    Read initial authoritative state -> Drain & process queued uevents -> Reconcile
    let initial_screen = get_screen_state();
    state.reduce(
        LoopEvent::ReconcileScreen(initial_screen),
        Instant::now(),
        min_screen_off_secs,
        maintenance_interval_secs,
        false,
    );

    if let Some(ref sock) = uevent_socket {
        let queued = sock.read_events();
        for ev in queued {
            let actions = state.reduce(
                LoopEvent::Uevent(ev),
                Instant::now(),
                min_screen_off_secs,
                maintenance_interval_secs,
                false,
            );
            for action in actions {
                if action == LoopAction::ReconcileSysfs {
                    let reconciled = get_screen_state();
                    state.reduce(
                        LoopEvent::ReconcileScreen(reconciled),
                        Instant::now(),
                        min_screen_off_secs,
                        maintenance_interval_secs,
                        false,
                    );
                }
            }
        }
    }

    arm_timer(timer_fd, state.earliest_deadline());
    log::info!("Daemon epoll event loop running with zero-idle overhead (timerfd one-shot).");

    let ctx_clone = ctx.clone();
    let ipc_handler = Arc::new(move |cmd: Command| ctx_clone.handle_command(cmd));
    let mut epoll_events: Vec<libc::epoll_event> = vec![unsafe { std::mem::zeroed() }; 16];

    'event_loop: loop {
        if ctx.is_shutdown_requested() {
            break;
        }

        // Blocking wait with no timeout — zero idle CPU wake-ups
        let nfds = unsafe {
            libc::epoll_wait(
                epoll_fd,
                epoll_events.as_mut_ptr(),
                epoll_events.len() as libc::c_int,
                -1,
            )
        };

        if nfds < 0 {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            if errno == libc::EINTR {
                continue;
            }
            log::error!("epoll_wait failed: {}", std::io::Error::last_os_error());
            break;
        }

        let mut timer_fired = false;
        let mut uevent_triggered = false;
        let mut signal_triggered = false;
        let mut ipc_triggered = false;
        let mut psi_level: Option<PsiPressureLevel> = None;

        for event in epoll_events.iter().take(nfds as usize) {
            let fd = event.u64 as i32;
            if fd == timer_fd {
                timer_fired = true;
            } else if Some(fd) == uevent_raw_fd {
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

        let now = Instant::now();
        let (maint_secs, min_off_secs) = {
            let cfg = ctx.config.read().unwrap_or_else(std::sync::PoisonError::into_inner);
            (cfg.maintenance_interval_secs, cfg.min_screen_off_secs)
        };
        let is_cleaning = ctx.is_cleaning();

        // Collect events to reduce
        let mut events_to_process = Vec::new();

        if timer_fired {
            drain_timer(timer_fd);
            events_to_process.push(LoopEvent::TimerExpired);
        }

        if signal_triggered {
            if let Some(ref sw) = signal_watcher {
                for sig in sw.read_events() {
                    events_to_process.push(LoopEvent::Signal(sig));
                }
            }
        }

        if ipc_triggered {
            events_to_process.push(LoopEvent::IpcReadable);
        }

        if let Some(lvl) = psi_level {
            events_to_process.push(LoopEvent::PsiPressure(lvl));
        }

        if uevent_triggered {
            if let Some(ref sock) = uevent_socket {
                let uevents = sock.read_events();
                if uevents.is_empty() {
                    events_to_process.push(LoopEvent::UeventBufferOverflow);
                } else {
                    for ev in uevents {
                        events_to_process.push(LoopEvent::Uevent(ev));
                    }
                }
            }
        }

        // Process all events through the pure reducer
        for ev in events_to_process {
            let actions = state.reduce(ev, now, min_off_secs, maint_secs, is_cleaning);

            for action in actions {
                match action {
                    LoopAction::None => {}

                    LoopAction::Shutdown => {
                        log::info!("Shutdown signal received. Stopping daemon...");
                        ctx.trigger_shutdown();
                        break 'event_loop;
                    }

                    LoopAction::ReloadConfig => {
                        log::info!("SIGHUP received: reloading configuration...");
                        let _ = ctx.reload_config();
                    }

                    LoopAction::HandleIpc => {
                        if let Some(server) = ipc_server {
                            server.accept_and_handle(ipc_handler.clone());
                        }
                    }

                    LoopAction::PreemptCleaning(reason) => {
                        log::info!("Preempting cleaning operation: {reason}");
                        ctx.cancel_token.cancel_with_reason(crate::engine::cancellation::CancelReason::ScreenOn);
                        ctx.set_state(DaemonState::Preempted(reason));
                    }

                    LoopAction::ReconcileSysfs => {
                        let sysfs_screen = get_screen_state();
                        let sub_actions = state.reduce(
                            LoopEvent::ReconcileScreen(sysfs_screen),
                            now,
                            min_off_secs,
                            maint_secs,
                            ctx.is_cleaning(),
                        );
                        for sa in sub_actions {
                            if let LoopAction::PreemptCleaning(r) = sa {
                                log::info!("Preempting cleaning operation (reconciled): {r}");
                                ctx.cancel_token.cancel_with_reason(crate::engine::cancellation::CancelReason::ScreenOn);
                                ctx.set_state(DaemonState::Preempted(r));
                            }
                        }
                    }

                    LoopAction::ReclaimMemory(level) => {
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
                                log::warn!("PSI memory stall (Level: {level}) - reclaiming memory...");

                                match level {
                                    PsiPressureLevel::Moderate => {
                                        let _ = crate::engine::memory::MemoryOptimizer::reclaim_cgroup_memory(64);
                                    }
                                    PsiPressureLevel::Critical => {
                                        let _ = crate::engine::memory::MemoryOptimizer::reclaim_cgroup_memory(128);
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

                    LoopAction::RunMaintenance => {
                        let should_run = ctx.evaluate_triggers(state.screen_off_since);
                        if should_run && !ctx.is_cleaning() && !ctx.is_shutdown_requested() {
                            ctx.spawn_maintenance_worker();
                            state.on_maintenance_success(now, maint_secs);
                        } else if !ctx.is_shutdown_requested() {
                            log::debug!("Maintenance postponed: triggers not satisfied. Backing off.");
                            state.on_maintenance_postponed(now, maint_secs);
                        }
                    }
                }
            }
        }

        // Re-arm timerfd to the earliest deadline before next wait
        if !ctx.is_shutdown_requested() {
            arm_timer(timer_fd, state.earliest_deadline());
        }
    }

    unsafe {
        libc::close(timer_fd);
        libc::close(epoll_fd);
    }
    log::info!("Epoll event loop terminated.");
}

// ─────────────────────────────────────────────────────────────────────────────
// Fallback loop (non-Unix / non-Epoll)
// ─────────────────────────────────────────────────────────────────────────────

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
