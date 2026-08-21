pub fn set_idle_priorities() {
    #[cfg(unix)]
    {
        // 1. Set CPU nice value to +19 (Lowest CPU priority)
        unsafe {
            libc::setpriority(libc::PRIO_PROCESS, 0, 19);
        }

        // 2. Set SCHED_BATCH or SCHED_IDLE if supported by kernel
        let param = libc::sched_param { sched_priority: 0 };
        let res = unsafe { libc::sched_setscheduler(0, 5, &param) };
        if res < 0 {
            let _ = unsafe { libc::sched_setscheduler(0, 3, &param) };
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
            #[cfg(not(any(target_arch = "aarch64", target_arch = "arm", target_arch = "x86_64")))]
            const SYS_IOPRIO_SET: libc::c_long = 30;

            let _ = libc::syscall(SYS_IOPRIO_SET, IOPRIO_WHO_PROCESS, 0, ioprio_val);
        }

        log::debug!("Governor applied: CPU SCHED_IDLE / nice 19, I/O IOPRIO_CLASS_IDLE");
    }
}
