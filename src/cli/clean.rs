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
            Ok(Response::Success(ResponseData::JobAccepted { job_id, message })) => {
                if !json {
                    println!("[+] {}", message);
                    println!("[*] Tracking progress for job #{}...", job_id);
                }
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(150));
                    match IpcClient::connect_and_send(&Command::GetJobStatus(job_id)) {
                        Ok(Response::Success(ResponseData::JobStatus {
                            is_completed,
                            report,
                            state,
                            ..
                        })) => {
                            if is_completed {
                                if let Some(rep) = report {
                                    if json {
                                        if let Ok(json_str) = serde_json::to_string_pretty(&rep) {
                                            println!("{json_str}");
                                        }
                                    } else {
                                        println!("[+] Clean job #{} completed via Daemon IPC:", job_id);
                                        print_clean_report(&rep);
                                    }
                                } else if json {
                                    println!("{{\"job_id\": {}, \"status\": \"{}\"}}", job_id, state);
                                } else {
                                    eprintln!("[!] Clean job #{} finished with state: {}", job_id, state);
                                }
                                break;
                            }
                        }
                        Ok(Response::Error(err)) => {
                            if json {
                                println!("{{\"error\": \"{}\"}}", err);
                            } else {
                                eprintln!("[!] Error polling job #{}: {}", job_id, err);
                            }
                            break;
                        }
                        Err(e) => {
                            if json {
                                println!("{{\"error\": \"{}\"}}", e);
                            } else {
                                eprintln!("[!] Connection lost while polling job #{}: {}", job_id, e);
                            }
                            break;
                        }
                        _ => {}
                    }
                }
            }
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
            Ok(Response::Success(ResponseData::Message(msg))) => {
                if json {
                    println!("{{\"message\": \"{}\"}}", msg);
                } else {
                    println!("[+] Daemon message: {}", msg);
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
