use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Throttle mode based on system load, thermal state, and pressure
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThrottleMode {
    Normal,      // 500 ops/s
    Warm,        // 300 ops/s
    CpuPressure, // 150 ops/s
    IoPressure,  // 50 ops/s
    Paused,      // 0 ops/s (fully paused)
}

impl ThrottleMode {
    #[inline]
    pub fn target_ops_per_sec(&self) -> u32 {
        match self {
            ThrottleMode::Normal => 500,
            ThrottleMode::Warm => 300,
            ThrottleMode::CpuPressure => 150,
            ThrottleMode::IoPressure => 50,
            ThrottleMode::Paused => 0,
        }
    }
}

/// Token bucket work-rate limiter to throttle walker filesystem operations
pub struct TokenBucketRateLimiter {
    state: Mutex<BucketState>,
}

struct BucketState {
    ops_per_sec: u32,
    capacity: f64,
    tokens: f64,
    last_refill: Instant,
}

impl TokenBucketRateLimiter {
    pub const MAX_BURST_CAPACITY: f64 = 32.0;

    pub fn new(initial_mode: ThrottleMode) -> Self {
        let rate = initial_mode.target_ops_per_sec();
        let capacity = (rate as f64).min(Self::MAX_BURST_CAPACITY).max(1.0);
        Self {
            state: Mutex::new(BucketState {
                ops_per_sec: rate,
                capacity,
                tokens: capacity, // Initial bounded burst
                last_refill: Instant::now(),
            }),
        }
    }

    /// Dynamically update throttle rate with anti-windfall normalization
    pub fn set_mode(&self, mode: ThrottleMode) {
        let new_rate = mode.target_ops_per_sec();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let new_capacity = (new_rate as f64).min(Self::MAX_BURST_CAPACITY).max(1.0);
        state.ops_per_sec = new_rate;
        state.capacity = new_capacity;
        // Clamp tokens to new capacity to prevent burst windfall
        if state.tokens > new_capacity {
            state.tokens = new_capacity;
        }
    }

    /// Acquire 1 work unit, blocking if bucket is empty until refilled.
    /// Returns false if rate is 0 (paused).
    pub fn acquire(&self) -> bool {
        loop {
            let sleep_dur = {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.ops_per_sec == 0 {
                    return false; // Paused
                }

                let now = Instant::now();
                let elapsed = now.duration_since(state.last_refill).as_secs_f64();
                state.last_refill = now;

                // Refill tokens
                state.tokens = (state.tokens + elapsed * (state.ops_per_sec as f64)).min(state.capacity);

                if state.tokens >= 1.0 {
                    state.tokens -= 1.0;
                    return true;
                }

                // Calculate required sleep duration for 1 token
                let missing = 1.0 - state.tokens;
                Duration::from_secs_f64((missing / (state.ops_per_sec as f64)).max(0.001))
            };

            std::thread::sleep(sleep_dur);
        }
    }
}
