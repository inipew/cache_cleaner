use super::output::print_clean_report;
use crate::config::DaemonConfig;
use crate::engine::{CancellationToken, CleanEngine};
use crate::ipc::{CleanParams, Command, IpcClient, Response, ResponseData};
use crate::system::{get_running_pid, PidLock};

pub fn execute_clean(
    config: DaemonConfig,
    deep: bool,
    trim: bool,
    zram: bool,
    dry_run: bool,
    json: bool,
) {
    let params = CleanParams {
        deep,
        trim: trim || deep,
        zram_compact: zram || deep,
        dry_run,
    };

    let pid_opt = get_running_pid();

    if let Some(pid) = pid_opt {
        if !json {
            println!(
                "[*] Active daemon detected (PID: {}). Delegating clean job via IPC...",
                pid
            );
        }
        match IpcClient::connect_and_send(&Command::TriggerClean(params)) {
            Ok(Response::Success(ResponseData::Report(report))) => {
                if json {
                    if let Ok(json_str) = serde_json::to_string_pretty(&report) {
                        println!("{json_str}");
                    }
                } else {
                    println!("[+] Clean job executed via Daemon IPC:");
                    print_clean_report(&report);
                }
            }
            Ok(Response::Error(msg)) => {
                if json {
                    println!("{{\"error\": \"{}\"}}", msg);
                } else {
                    eprintln!("[!] Daemon returned error: {}", msg);
                }
            }
            Err(e) => {
                if json {
                    println!("{{\"error\": \"{}\"}}", e);
                } else {
                    eprintln!(
                        "[!] Failed to communicate with daemon (PID: {}): {}",
                        pid, e
                    );
                }
            }
            _ => {
                if json {
                    println!("{{\"error\": \"Unexpected response from daemon\"}}");
                } else {
                    eprintln!("[!] Unexpected response from daemon");
                }
            }
        }
    } else {
        if !json {
            println!("[*] Daemon is offline. Running standalone clean mode...");
        }
        let _pid_lock = match PidLock::acquire() {
            Ok(lock) => lock,
            Err(e) => {
                if json {
                    println!("{{\"error\": \"{}\"}}", e);
                } else {
                    eprintln!("[!] Cannot run standalone clean: {}", e);
                }
                return;
            }
        };

        // Apply low priorities and background cgroup
        let _ = crate::system::migrate_to_background_cgroup();
        crate::system::governor::set_idle_priorities();

        let cancel_token = CancellationToken::new();
        let mut engine = CleanEngine::new(config);
        match engine.execute(&params, &cancel_token) {
            Ok(report) => {
                if json {
                    if let Ok(json_str) = serde_json::to_string_pretty(&report) {
                        println!("{json_str}");
                    }
                } else {
                    println!("[+] Standalone Clean Completed:");
                    print_clean_report(&report);
                }
            }
            Err(e) => {
                eprintln!("[-] Clean execution failed: {e}");
            }
        }
    }
}
