#[cfg(test)]
mod tests {
    use cache_cleaner_daemon::util::{ThrottleMode, TokenBucketRateLimiter};

    #[test]
    fn test_token_bucket_bounded_burst_and_pause() {
        let limiter = TokenBucketRateLimiter::new(ThrottleMode::Normal);

        // Initial burst must not exceed MAX_BURST_CAPACITY (32 tokens)
        for _ in 0..32 {
            assert!(limiter.acquire());
        }

        // Dynamic change to Paused mode
        limiter.set_mode(ThrottleMode::Paused);
        assert!(!limiter.acquire());

        // Restore to Warm mode
        limiter.set_mode(ThrottleMode::Warm);
        assert!(limiter.acquire());
    }
}
