#[cfg(test)]
mod tests {
    use cache_cleaner_daemon::engine::storage::{
        mark_fstrim_completed, record_freed_bytes_for_trim, should_run_fstrim,
    };

    #[test]
    fn test_fstrim_delta_and_cooldown_logic() {
        // 1. Mark trim completed just now (sets last trim to current timestamp, delta = 0)
        mark_fstrim_completed();

        // Immediate automatic run without delta should be skipped (cooldown active)
        assert!(!should_run_fstrim(false));

        // Manual request bypasses cooldown
        assert!(should_run_fstrim(true));

        // 2. Accumulate 300 MB (< 500 MB threshold) -> still skipped
        record_freed_bytes_for_trim(300_000_000);
        assert!(!should_run_fstrim(false));

        // 3. Accumulate another 250 MB (total 550 MB >= 500 MB) -> eligible!
        record_freed_bytes_for_trim(250_000_000);
        assert!(should_run_fstrim(false));

        // 4. Mark trim completed -> resets delta and restarts cooldown
        mark_fstrim_completed();
        assert!(!should_run_fstrim(false));
    }
}
