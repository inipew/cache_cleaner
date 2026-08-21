use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use crate::config::DaemonConfig;
use crate::ipc::{Command, IpcClient, Response, ResponseData};
use crate::system::{clean_stale_pid_files, get_running_pid, is_process_alive, DaemonRunner};

pub fn start_daemon(config_path: Option<&PathBuf>, foreground: bool) {
    if let Some(pid) = get_running_pid() {
        println!("[!] Cleaner daemon is already running (PID: {}).", pid);
        return;
    }

    if foreground {
        let (cfg, path) = DaemonConfig::load_or_default_with_path(config_path);
        let mut runner = DaemonRunner::new(cfg, path);
        runner.run();
        return;
    }

    let exe = if std::path::Path::new(crate::config::BIN_PATH).exists() {
        PathBuf::from(crate::config::BIN_PATH)
    } else {
        match std::env::current_exe() {
            Ok(e) => e,
            Err(err) => {
                eprintln!("[!] Failed to locate binary executable: {}", err);
                return;
            }
        }
    };

    let mut cmd = std::process::Command::new(exe);
    cmd.arg("daemon");
    if let Some(p) = config_path {
        cmd.arg("--config").arg(p);
    }

    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    match cmd.spawn() {
        Ok(child) => {
            let child_pid = child.id();
            println!("[*] Spawning background daemon (PID: {})...", child_pid);

            // Wait up to 2.5 seconds for IPC readiness
            let start = Instant::now();
            let mut ready = false;
            while start.elapsed() < Duration::from_millis(2500) {
                thread::sleep(Duration::from_millis(150));
                if let Ok(Response::Success(ResponseData::Pong { .. })) =
                    IpcClient::connect_and_send(&Command::Ping)
                {
                    ready = true;
                    break;
                }
            }

            if ready {
                println!(
                    "[+] Cleaner daemon started successfully (PID: {}).",
                    child_pid
                );
            } else if is_process_alive(child_pid) {
                println!("[+] Cleaner daemon is running (PID: {}).", child_pid);
            } else {
                eprintln!("[!] Daemon process exited prematurely. Check system logs for details.");
            }
        }
        Err(e) => {
            eprintln!("[!] Failed to start background daemon: {}", e);
        }
    }
}

pub fn stop_daemon() {
    let pid_opt = get_running_pid();

    if pid_opt.is_none() {
        // Also test IPC ping
        if IpcClient::connect_and_send(&Command::Ping).is_err() {
            println!("[*] Cleaner daemon is not running.");
            clean_stale_pid_files();
            return;
        }
    }

    println!("[*] Stopping cleaner daemon...");

    // 1. Tier 1: Graceful IPC Stop
    let _ = IpcClient::connect_and_send(&Command::StopDaemon);

    // 2. Wait up to 1.5s for graceful stop
    if let Some(pid) = pid_opt {
        let wait_start = Instant::now();
        while is_process_alive(pid) && wait_start.elapsed() < Duration::from_millis(1500) {
            thread::sleep(Duration::from_millis(100));
        }

        // 3. Tier 2: SIGTERM if still alive
        if is_process_alive(pid) {
            log::warn!(
                "Daemon did not exit via IPC, sending SIGTERM to PID {}",
                pid
            );
            #[cfg(unix)]
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGTERM);
            }

            let term_start = Instant::now();
            while is_process_alive(pid) && term_start.elapsed() < Duration::from_millis(1500) {
                thread::sleep(Duration::from_millis(100));
            }
        }

        // 4. Tier 3: SIGKILL if still alive
        if is_process_alive(pid) {
            log::warn!(
                "Daemon unresponsive to SIGTERM, sending SIGKILL to PID {}",
                pid
            );
            #[cfg(unix)]
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGKILL);
            }
            thread::sleep(Duration::from_millis(200));
        }

        // 5. Prevent Android init from auto-respawning if running as an init service
        let _ = std::process::Command::new("setprop")
            .args(["ctl.stop", "cleaner_daemon"])
            .output();

        if !is_process_alive(pid) {
            println!("[+] Cleaner daemon (PID: {}) stopped completely.", pid);
        } else {
            eprintln!("[!] Warning: Could not terminate PID {}.", pid);
        }
    } else {
        println!("[+] Stop signal sent to cleaner daemon.");
    }

    clean_stale_pid_files();
}

pub fn restart_daemon(config_path: Option<&PathBuf>, foreground: bool) {
    println!("[*] Restarting Android Cache Cleaner Daemon...");
    stop_daemon();
    thread::sleep(Duration::from_millis(300));
    start_daemon(config_path, foreground);
}

pub fn cancel_operation() {
    match IpcClient::connect_and_send(&Command::Cancel) {
        Ok(Response::Success(ResponseData::Message(msg))) => {
            println!("[+] {}", msg);
        }
        Err(e) => eprintln!("Failed to cancel operation: {}", e),
        _ => {}
    }
}

pub fn reload_config() {
    // 1. Try sending IPC reload command
    match IpcClient::connect_and_send(&Command::ReloadConfig) {
        Ok(Response::Success(ResponseData::Message(msg))) => {
            println!("[+] {}", msg);
            return;
        }
        Ok(Response::Error(err)) => {
            eprintln!("[!] Config reload rejected: {}", err);
            return;
        }
        _ => {}
    }

    // 2. Fallback: Send SIGHUP to running PID
    if let Some(pid) = get_running_pid() {
        #[cfg(unix)]
        {
            let res = unsafe { libc::kill(pid as libc::pid_t, libc::SIGHUP) };
            if res == 0 {
                println!("[+] Sent SIGHUP reload signal to daemon (PID: {})", pid);
                return;
            }
        }
    }

    eprintln!("[!] Failed to reload: Cleaner daemon is not running.");
}
