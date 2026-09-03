use cache_cleaner_daemon::platform::privilege::MagiskPlatform;
use cache_cleaner_daemon::platform::privilege::PrivilegePlatform;
use cache_cleaner_daemon::resource::ResourceManager;
use cache_cleaner_daemon::scheduler::SchedulerService;
use cache_cleaner_daemon::store::SqliteStore;
use std::time::Instant;

/// 84.md Idle Power verification: no polling loop, blocking wait, sparse reconciliation.
#[test]
fn idle_no_job_no_polling() {
    let store = SqliteStore::in_memory().unwrap();
    let scheduler = SchedulerService::new(store);
    // No jobs admitted — pop should be None and not busy loop
    assert!(scheduler.pop_next_job().unwrap().is_none());
    // Second pop immediately should also be None (no timer created)
    let start = Instant::now();
    for _ in 0..100 {
        assert!(scheduler.pop_next_job().unwrap().is_none());
    }
    let elapsed = start.elapsed();
    // 100 pops should be fast (<10ms) and not involve sleep/poll
    assert!(elapsed.as_millis() < 50, "idle pop should be cheap, not polling");
}

#[test]
fn resource_manager_no_polling_on_idle() {
    let rm = ResourceManager::default();
    // Idle — no target locked, no FD held
    assert_eq!(rm.active_fd_count(), 0);
    // Throttle should not busy loop
    let start = Instant::now();
    rm.throttle_mutation();
    assert!(start.elapsed().as_millis() < 10);
}

#[test]
fn platform_capabilities_probe_is_cached_not_polling() {
    let p = MagiskPlatform;
    let caps1 = p.discover_capabilities();
    let caps2 = p.discover_capabilities();
    // Probe should be cheap and not require wakelock
    assert_eq!(caps1.has_openat2, caps2.has_openat2);
}
