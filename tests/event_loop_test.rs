#[cfg(test)]
#[cfg(unix)]
mod tests {
    use cache_cleaner_daemon::hardware::{ScreenState, UeventMessage};
    use cache_cleaner_daemon::system::{
        instant_to_itimerspec, LoopAction, LoopEvent, LoopState, SignalEvent,
    };
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    // ─────────────────────────────────────────────────────────────────────────
    // 1. Invariant & Property Tests (Pure State Machine)
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_screen_on_has_no_screen_off_deadline() {
        let now = Instant::now();
        let mut state = LoopState::new(3600, now);

        let actions = state.reduce(
            LoopEvent::ReconcileScreen(ScreenState::On),
            now,
            180,
            3600,
            false,
        );

        assert_eq!(state.screen_state, ScreenState::On);
        assert_eq!(state.screen_off_since, None);
        assert_eq!(state.screen_off_deadline, None);
        assert!(actions.is_empty());
    }

    #[test]
    fn test_screen_off_sets_screen_off_deadline() {
        let now = Instant::now();
        let mut state = LoopState::new(3600, now);

        let min_off = 180;
        let actions = state.reduce(
            LoopEvent::ReconcileScreen(ScreenState::Off),
            now,
            min_off,
            3600,
            false,
        );

        assert_eq!(state.screen_state, ScreenState::Off);
        assert_eq!(state.screen_off_since, Some(now));
        assert_eq!(
            state.screen_off_deadline,
            Some(now + Duration::from_secs(min_off))
        );
        assert!(actions.is_empty());
    }

    #[test]
    fn test_screen_on_cancels_screen_off_deadline_and_preempts_if_cleaning() {
        let now = Instant::now();
        let mut state = LoopState::new(3600, now);

        // Screen is initially off
        state.reduce(
            LoopEvent::ReconcileScreen(ScreenState::Off),
            now,
            180,
            3600,
            false,
        );
        assert!(state.screen_off_deadline.is_some());

        // Screen turns ON while cleaning is active
        let t1 = now + Duration::from_secs(60);
        let actions = state.reduce(
            LoopEvent::ReconcileScreen(ScreenState::On),
            t1,
            180,
            3600,
            true, // is_cleaning = true
        );

        assert_eq!(state.screen_state, ScreenState::On);
        assert_eq!(state.screen_off_deadline, None);
        assert_eq!(state.screen_off_since, None);
        assert_eq!(
            actions,
            vec![LoopAction::PreemptCleaning("Screen turned ON".to_string())]
        );
    }

    #[test]
    fn test_earliest_deadline_selection() {
        let now = Instant::now();
        let mut state = LoopState::new(3600, now); // maintenance at now + 3600s

        // 1. When only maintenance deadline exists
        assert_eq!(
            state.earliest_deadline(),
            now + Duration::from_secs(3600)
        );

        // 2. When screen_off_deadline is sooner (now + 180s < now + 3600s)
        state.screen_off_deadline = Some(now + Duration::from_secs(180));
        assert_eq!(
            state.earliest_deadline(),
            now + Duration::from_secs(180)
        );

        // 3. When retry_deadline is sooner than both (now + 30s)
        state.retry_deadline = Some(now + Duration::from_secs(30));
        assert_eq!(
            state.earliest_deadline(),
            now + Duration::from_secs(30)
        );

        // 4. When maintenance deadline is sooner than screen_off_deadline
        state.retry_deadline = None;
        state.maintenance_deadline = now + Duration::from_secs(60);
        state.screen_off_deadline = Some(now + Duration::from_secs(180));
        assert_eq!(
            state.earliest_deadline(),
            now + Duration::from_secs(60)
        );
    }

    #[test]
    fn test_uevent_error_triggers_reconciliation() {
        let now = Instant::now();
        let mut state = LoopState::new(3600, now);
        assert!(state.uevent_ok);

        let actions = state.reduce(
            LoopEvent::UeventBufferOverflow,
            now,
            180,
            3600,
            false,
        );

        assert!(!state.uevent_ok);
        assert_eq!(actions, vec![LoopAction::ReconcileSysfs]);
    }

    #[test]
    fn test_display_uevent_triggers_sysfs_reconciliation() {
        let now = Instant::now();
        let mut state = LoopState::new(3600, now);

        let uevent = UeventMessage {
            action: "change".to_string(),
            devpath: "/devices/platform/backlight".to_string(),
            subsystem: "backlight".to_string(),
            properties: HashMap::new(),
        };

        let actions = state.reduce(
            LoopEvent::Uevent(uevent),
            now,
            180,
            3600,
            false,
        );

        assert_eq!(actions, vec![LoopAction::ReconcileSysfs]);
    }

    #[test]
    fn test_exponential_retry_backoff_and_reset() {
        let now = Instant::now();
        let mut state = LoopState::new(3600, now);

        assert_eq!(state.retry_backoff_secs, 5);

        // First failure: backs off 5s, next backoff is 10s
        state.on_maintenance_postponed(now, 3600);
        assert_eq!(state.retry_deadline, Some(now + Duration::from_secs(5)));
        assert_eq!(state.retry_backoff_secs, 10);

        // Second failure: backs off 10s, next backoff is 20s
        let t1 = now + Duration::from_secs(5);
        state.on_maintenance_postponed(t1, 3600);
        assert_eq!(state.retry_deadline, Some(t1 + Duration::from_secs(10)));
        assert_eq!(state.retry_backoff_secs, 20);

        // Third failure: backs off 20s, next backoff is 40s
        let t2 = t1 + Duration::from_secs(10);
        state.on_maintenance_postponed(t2, 3600);
        assert_eq!(state.retry_deadline, Some(t2 + Duration::from_secs(20)));
        assert_eq!(state.retry_backoff_secs, 40);

        // Fourth failure: backs off 40s, next backoff is capped at 60s
        let t3 = t2 + Duration::from_secs(20);
        state.on_maintenance_postponed(t3, 3600);
        assert_eq!(state.retry_deadline, Some(t3 + Duration::from_secs(40)));
        assert_eq!(state.retry_backoff_secs, 60);

        // Fifth failure: capped at 60s
        let t4 = t3 + Duration::from_secs(40);
        state.on_maintenance_postponed(t4, 3600);
        assert_eq!(state.retry_deadline, Some(t4 + Duration::from_secs(60)));
        assert_eq!(state.retry_backoff_secs, 60);

        // Maintenance success resets retry backoff
        let t5 = t4 + Duration::from_secs(60);
        state.on_maintenance_success(t5, 3600);
        assert_eq!(state.retry_deadline, None);
        assert_eq!(state.retry_backoff_secs, 5);
    }

    #[test]
    fn test_signal_reload_reschedules_maintenance_deadline() {
        let now = Instant::now();
        let mut state = LoopState::new(3600, now);
        state.retry_deadline = Some(now + Duration::from_secs(30));

        let t1 = now + Duration::from_secs(500);
        let actions = state.reduce(
            LoopEvent::Signal(SignalEvent::Reload),
            t1,
            180,
            7200, // new interval
            false,
        );

        assert_eq!(actions, vec![LoopAction::ReloadConfig]);
        assert_eq!(state.maintenance_deadline, t1 + Duration::from_secs(7200));
        assert_eq!(state.retry_deadline, None);
        assert_eq!(state.retry_backoff_secs, 5);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // 2. Instant to itimerspec Conversion Unit Tests
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_instant_to_itimerspec_future_deadline() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(10);

        let itimer = instant_to_itimerspec(deadline, now);
        assert_eq!(itimer.it_interval.tv_sec, 0);
        assert_eq!(itimer.it_interval.tv_nsec, 0);

        let mut mono_now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut mono_now) };

        // Value must be approximately mono_now + 10s
        assert!(itimer.it_value.tv_sec >= mono_now.tv_sec + 9);
        assert!(itimer.it_value.tv_sec <= mono_now.tv_sec + 11);
    }

    #[test]
    fn test_instant_to_itimerspec_past_deadline_fires_immediately() {
        let now = Instant::now();
        let past_deadline = now - Duration::from_secs(10);

        let itimer = instant_to_itimerspec(past_deadline, now);
        // Past deadline must be positive (>= 1ns) so timer is armed and not disarmed (0)
        assert!(itimer.it_value.tv_sec > 0 || itimer.it_value.tv_nsec > 0);
    }

    #[test]
    fn test_instant_to_itimerspec_exact_now_deadline() {
        let now = Instant::now();
        let itimer = instant_to_itimerspec(now, now);
        assert!(itimer.it_value.tv_sec > 0 || itimer.it_value.tv_nsec > 0);
    }

    #[test]
    fn test_instant_to_itimerspec_nanosecond_overflow_carry() {
        let now = Instant::now();
        let deadline = now + Duration::from_nanos(999_999_999);

        let itimer = instant_to_itimerspec(deadline, now);
        assert!(itimer.it_value.tv_nsec >= 0 && itimer.it_value.tv_nsec < 1_000_000_000);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // 3. Kernel Integration Tests (timerfd & epoll)
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_timerfd_create_arm_and_drain() {
        let timer_fd = unsafe {
            libc::timerfd_create(libc::CLOCK_MONOTONIC, libc::TFD_NONBLOCK | libc::TFD_CLOEXEC)
        };
        assert!(timer_fd >= 0, "timerfd_create failed");

        let mut mono_now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut mono_now) };

        let delay_nsec = 10_000_000u64; // 10ms
        let total_nsec = mono_now.tv_nsec as u64 + delay_nsec;
        let abs_ts = libc::timespec {
            tv_sec: mono_now.tv_sec + (total_nsec / 1_000_000_000) as i64,
            tv_nsec: (total_nsec % 1_000_000_000) as i64,
        };

        let new_value = libc::itimerspec {
            it_interval: libc::timespec { tv_sec: 0, tv_nsec: 0 },
            it_value: abs_ts,
        };

        let ret = unsafe {
            libc::timerfd_settime(timer_fd, libc::TFD_TIMER_ABSTIME, &new_value, std::ptr::null_mut())
        };
        assert_eq!(ret, 0, "timerfd_settime failed");

        std::thread::sleep(Duration::from_millis(20));

        let mut buf = [0u8; 8];
        let n = unsafe { libc::read(timer_fd, buf.as_mut_ptr() as *mut libc::c_void, 8) };
        assert_eq!(n, 8, "Expected 8 bytes read from timerfd");

        let expirations = u64::from_ne_bytes(buf);
        assert!(expirations >= 1, "Expected at least 1 expiration");

        // Drain again non-blocking -> EAGAIN
        let n2 = unsafe { libc::read(timer_fd, buf.as_mut_ptr() as *mut libc::c_void, 8) };
        assert!(n2 < 0);
        let err = std::io::Error::last_os_error();
        assert_eq!(err.raw_os_error(), Some(libc::EAGAIN));

        unsafe { libc::close(timer_fd) };
    }

    #[test]
    fn test_epoll_with_timerfd_one_shot() {
        let epoll_fd = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
        assert!(epoll_fd >= 0);

        let timer_fd = unsafe {
            libc::timerfd_create(libc::CLOCK_MONOTONIC, libc::TFD_NONBLOCK | libc::TFD_CLOEXEC)
        };
        assert!(timer_fd >= 0);

        let mut ev = libc::epoll_event {
            events: libc::EPOLLIN as u32,
            u64: timer_fd as u64,
        };
        let ctl_res = unsafe { libc::epoll_ctl(epoll_fd, libc::EPOLL_CTL_ADD, timer_fd, &mut ev) };
        assert_eq!(ctl_res, 0);

        let mut mono_now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut mono_now) };

        let total_nsec = mono_now.tv_nsec as u64 + 15_000_000;
        let abs_ts = libc::timespec {
            tv_sec: mono_now.tv_sec + (total_nsec / 1_000_000_000) as i64,
            tv_nsec: (total_nsec % 1_000_000_000) as i64,
        };

        let new_value = libc::itimerspec {
            it_interval: libc::timespec { tv_sec: 0, tv_nsec: 0 },
            it_value: abs_ts,
        };
        unsafe {
            libc::timerfd_settime(timer_fd, libc::TFD_TIMER_ABSTIME, &new_value, std::ptr::null_mut())
        };

        let mut events: [libc::epoll_event; 4] = [unsafe { std::mem::zeroed() }; 4];
        let start = Instant::now();
        let nfds = unsafe { libc::epoll_wait(epoll_fd, events.as_mut_ptr(), 4, -1) };
        let elapsed = start.elapsed();

        assert_eq!(nfds, 1);
        assert_eq!(events[0].u64 as i32, timer_fd);
        assert!(elapsed >= Duration::from_millis(10));

        let mut buf = [0u8; 8];
        unsafe { libc::read(timer_fd, buf.as_mut_ptr() as *mut libc::c_void, 8) };

        unsafe {
            libc::close(timer_fd);
            libc::close(epoll_fd);
        }
    }
}

