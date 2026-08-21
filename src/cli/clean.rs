use super::output::print_clean_report;
use crate::config::DaemonConfig;
use crate::engine::{CancellationToken, CleanEngine};
use crate::ipc::{CleanParams, Command, IpcClient, Response, ResponseData};
use crate::system::{get_running_pid, PidLock};

pub fn execute_clean(config: DaemonConfig, deep: bool, trim: bool, zram: bool, dry_run: bool) {
    let params = CleanParams {
        deep,
        trim: trim || deep,
        zram_compact: zram || deep,
        dry_run,
    };

    let pid_opt = get_running_pid();

    if let Some(pid) = pid_opt {
        println!(
            "[*] Active daemon detected (PID: {}). Delegating clean job via IPC...",
            pid
        );
        match IpcClient::connect_and_send(&Command::TriggerClean(params)) {
            Ok(Response::Success(ResponseData::Report(report))) => {
                println!("[+] Clean job executed via Daemon IPC:");
                print_clean_report(&report);
            }
            Ok(Response::Error(msg)) => {
                eprintln!("[!] Daemon returned error: {}", msg);
            }
            Err(e) => {
                eprintln!(
                    "[!] Failed to communicate with daemon (PID: {}): {}",
                    pid, e
                );
            }
            _ => {
                eprintln!("[!] Unexpected response from daemon");
            }
        }
    } else {
        println!("[*] Daemon is offline. Running standalone clean mode...");
        let _pid_lock = match PidLock::acquire() {
            Ok(lock) => lock,
            Err(e) => {
                eprintln!("[!] Cannot run standalone clean: {}", e);
                return;
            }
        };

        // Apply low priorities and background cgroup
        crate::system::migrate_to_background_cgroup();
        crate::system::governor::set_idle_priorities();

        let cancel_token = CancellationToken::new();
        let engine = CleanEngine::new(config);
        let report = engine.execute(&params, &cancel_token);

        println!("[+] Standalone Clean Completed:");
        print_clean_report(&report);
    }
}
