pub fn set_idle_priorities() {
    #[cfg(unix)]
    {
        // 1. Set CPU nice value to +19 (Lowest CPU priority)
        let nice_res = unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, 19) };
        if nice_res < 0 {
            log::debug!("Failed to set CPU nice 19 (errno: {})", std::io::Error::last_os_error());
        }

        // 2. Set SCHED_IDLE (5) or SCHED_BATCH (3) if supported by kernel
        let param = libc::sched_param { sched_priority: 0 };
        let sched_res = unsafe { libc::sched_setscheduler(0, 5, &param) };
        if sched_res < 0 {
            let batch_res = unsafe { libc::sched_setscheduler(0, 3, &param) };
            if batch_res < 0 {
                log::debug!("Scheduler policy adjustment not permitted or unsupported");
            }
        }

        // 3. Set I/O Priority to IOPRIO_CLASS_IDLE (Class 3)
        const IOPRIO_WHO_PROCESS: libc::c_int = 1;
        const IOPRIO_CLASS_IDLE: libc::c_int = 3;
        const IOPRIO_CLASS_SHIFT: libc::c_int = 13;
        let ioprio_val = (IOPRIO_CLASS_IDLE << IOPRIO_CLASS_SHIFT) | 7;

        unsafe {
            #[cfg(target_arch = "aarch64")]
            const SYS_IOPRIO_SET: libc::c_long = 30;
            #[cfg(target_arch = "arm")]
            const SYS_IOPRIO_SET: libc::c_long = 314;
            #[cfg(target_arch = "x86_64")]
            const SYS_IOPRIO_SET: libc::c_long = 251;
            #[cfg(target_arch = "x86")]
            const SYS_IOPRIO_SET: libc::c_long = 289;
            #[cfg(target_arch = "riscv64")]
            const SYS_IOPRIO_SET: libc::c_long = 30;
            #[cfg(not(any(target_arch = "aarch64", target_arch = "arm", target_arch = "x86_64", target_arch = "x86", target_arch = "riscv64")))]
            const SYS_IOPRIO_SET: libc::c_long = 30;

            let io_res = libc::syscall(SYS_IOPRIO_SET, IOPRIO_WHO_PROCESS, 0, ioprio_val);
            if io_res < 0 {
                log::debug!("ioprio_set returned errno: {}", std::io::Error::last_os_error());
            }
        }

        log::debug!("Governor idle priorities requested (nice 19 / SCHED_IDLE / IOPRIO_CLASS_IDLE)");
    }
}
