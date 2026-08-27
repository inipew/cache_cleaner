use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Abstract clock source for time-dependent operations.
pub trait Clock: Send + Sync {
    /// Monotonic time source (for grace periods, durations, deadlines, timeouts)
    fn now(&self) -> Instant;

    /// Wall-clock time source (for persistent timestamps and logging)
    fn system_time(&self) -> SystemTime;
}

/// Standard production clock backed by OS monotonic and real-time clocks.
#[derive(Debug, Clone, Default)]
pub struct RealClock;

impl Clock for RealClock {
    #[inline]
    fn now(&self) -> Instant {
        Instant::now()
    }

    #[inline]
    fn system_time(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// Deterministic mock clock for unit and integration testing without real sleeps.
#[derive(Debug, Clone)]
pub struct FakeClock {
    base_instant: Instant,
    offset_nanos: Arc<AtomicU64>,
    base_system_time: SystemTime,
}

impl FakeClock {
    pub fn new() -> Self {
        Self {
            base_instant: Instant::now(),
            offset_nanos: Arc::new(AtomicU64::new(0)),
            base_system_time: UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        }
    }

    pub fn with_system_time(sys_time: SystemTime) -> Self {
        Self {
            base_instant: Instant::now(),
            offset_nanos: Arc::new(AtomicU64::new(0)),
            base_system_time: sys_time,
        }
    }

    /// Advance simulated monotonic and wall time by a given duration
    pub fn advance(&self, duration: Duration) {
        let nanos = duration.as_nanos() as u64;
        self.offset_nanos.fetch_add(nanos, Ordering::SeqCst);
    }
}

impl Default for FakeClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Instant {
        let offset = Duration::from_nanos(self.offset_nanos.load(Ordering::SeqCst));
        self.base_instant + offset
    }

    fn system_time(&self) -> SystemTime {
        let offset = Duration::from_nanos(self.offset_nanos.load(Ordering::SeqCst));
        self.base_system_time + offset
    }
}
