#[cfg(test)]
mod tests {
    use cache_cleaner_daemon::system::{clean_stale_pid_files, is_process_alive, PidLock};

    #[test]
    fn test_pid_lock_acquisition_and_mutual_exclusion() {
        let lock1 = PidLock::acquire();
        assert!(lock1.is_ok(), "First PID lock acquisition should succeed");
        let l1 = lock1.as_ref().unwrap();
        assert!(!l1.path().as_os_str().is_empty());
        assert!(!l1.lock_path().as_os_str().is_empty());

        let lock2 = PidLock::acquire();
        assert!(
            lock2.is_err(),
            "Second PID lock acquisition must fail due to mutual exclusion"
        );

        // Drop lock1 to release the flock
        drop(lock1);

        let lock3 = PidLock::acquire();
        assert!(
            lock3.is_ok(),
            "PID lock acquisition after drop should succeed"
        );
        drop(lock3);

        clean_stale_pid_files();
    }

    #[test]
    fn test_process_alive_check() {
        let current_pid = std::process::id();
        assert!(is_process_alive(current_pid));
        assert!(!is_process_alive(0));
        // PID 999_999 is almost certainly non-existent
        assert!(!is_process_alive(999_999));
    }
}

