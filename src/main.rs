mod config;
mod engine;
mod error;
mod hardware;
mod ipc;
mod platform;
mod system;
mod util;

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use config::DaemonConfig;
use engine::{CancellationToken, CleanEngine};
use ipc::{CleanParams, Command, IpcClient, Response, ResponseData};
use platform::{check_encryption_state, enumerate_users, get_selinux_mode, AndroidSystemInfo};
use system::{clean_stale_pid_files, get_running_pid, is_process_alive, DaemonRunner, PidLock};
use util::{format_bytes, init_logger};

#[derive(Parser, Debug)]
#[command(
    name = "cache-cleaner",
    author = "Antigravity Developer",
    version = env!("CARGO_PKG_VERSION"),
    about = "Zero-Idle-Overhead Android Cache Cleaner & System Optimizer Daemon in Rust",
    long_about = "A high-performance, native ROM-friendly background cleaner daemon and CLI utility supporting Android 9 to 16."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Path to custom configuration file
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start the background cleaner daemon (if not already running)
    Start {
        /// Run in foreground instead of background detached process
        #[arg(short, long)]
        foreground: bool,
    },

    /// Run as background daemon directly (Used by Android init / supervisors)
    Daemon,

    /// Gracefully stop the running cleaner daemon (3-tier: IPC -> SIGTERM -> SIGKILL)
    Stop,

    /// Restart the cleaner daemon cleanly
    Restart {
        /// Run in foreground after restart
        #[arg(short, long)]
        foreground: bool,
    },

    /// Trigger a cache and junk cleaning pass (Uses IPC if daemon is active, else standalone)
    Clean {
        /// Perform deep cleaning (includes storage trimming and ZRAM compaction)
        #[arg(short, long)]
        deep: bool,

        /// Run FITRIM on mount points (/data, /cache)
        #[arg(short, long)]
        trim: bool,

        /// Compact ZRAM swap memory
        #[arg(short, long)]
        zram: bool,

        /// Dry-run mode: scan and calculate freed space without deleting files
        #[arg(short = 'n', long)]
        dry_run: bool,
    },

    /// Query daemon status via IPC or PID lock
    Status,

    /// Query accumulated cleaning statistics via IPC
    Stats {
        /// Output in raw JSON format
        #[arg(long)]
        json: bool,
    },

    /// Cancel an ongoing cleaning operation in the running daemon
    Cancel,

    /// Reload daemon configuration without restarting (via IPC or SIGHUP)
    Reload,

    /// Show detected Android platform, FBE encryption, users, and hardware info
    Info,

    /// Generate a default cleaner.toml configuration file
    ConfigInit {
        /// Destination path (defaults to ./cleaner.toml)
        #[arg(short, long, default_value = "cleaner.toml")]
        output: PathBuf,
    },
}

fn main() {
    init_logger();
    let cli = Cli::parse();

    let (config, active_config_path) = DaemonConfig::load_or_default_with_path(cli.config.as_ref());

    match cli.command.unwrap_or(Commands::Status) {
        Commands::Start { foreground } => {
            start_daemon(active_config_path.as_ref(), foreground);
        }

        Commands::Daemon => {
            let mut runner = DaemonRunner::new(config, active_config_path);
            runner.run();
        }

        Commands::Stop => {
            stop_daemon();
        }

        Commands::Restart { foreground } => {
            println!("[*] Restarting Android Cache Cleaner Daemon...");
            stop_daemon();
            thread::sleep(Duration::from_millis(300));
            start_daemon(active_config_path.as_ref(), foreground);
        }

        Commands::Clean {
            deep,
            trim,
            zram,
            dry_run,
        } => {
            let params = CleanParams {
                deep,
                trim: trim || deep,
                zram_compact: zram || deep,
                dry_run,
            };

            let pid_opt = get_running_pid();

            if let Some(pid) = pid_opt {
                println!("[*] Active daemon detected (PID: {}). Delegating clean job via IPC...", pid);
                match IpcClient::connect_and_send(&Command::TriggerClean(params)) {
                    Ok(Response::Success(ResponseData::Report(report))) => {
                        println!("[+] Clean job executed via Daemon IPC:");
                        print_clean_report(&report);
                    }
                    Ok(Response::Error(msg)) => {
                        eprintln!("[!] Daemon returned error: {}", msg);
                    }
                    Err(e) => {
                        eprintln!("[!] Failed to communicate with daemon (PID: {}): {}", pid, e);
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

                let cancel_token = CancellationToken::new();
                let engine = CleanEngine::new(config);
                let report = engine.execute(&params, &cancel_token);

                println!("[+] Standalone Clean Completed:");
                print_clean_report(&report);
            }
        }

        Commands::Status => {
            let pid_opt = get_running_pid();

            match IpcClient::connect_and_send(&Command::GetStatus) {
                Ok(Response::Success(ResponseData::Status(s))) => {
                    println!("==================================================");
                    println!("          ANDROID CACHE CLEANER DAEMON            ");
                    println!("==================================================");
                    println!("  PID           : {}", pid_opt.map(|p| p.to_string()).unwrap_or_else(|| "N/A".to_string()));
                    println!("  State         : {}", s.state);
                    println!("  Uptime        : {} seconds", s.uptime_secs);
                    println!("  CPU Usage     : {:.2}%", s.cpu_usage_pct);
                    println!("  RAM (PSS)     : {}", format_bytes(s.ram_pss_bytes));
                    println!("  RAM (RSS)     : {}", format_bytes(s.ram_rss_bytes));
                    println!("  RAM (Total/VM): {}", format_bytes(s.ram_vm_size_bytes));
                    println!("  Total Freed   : {}", format_bytes(s.total_freed_bytes));
                    println!("  Last Freed    : {}", format_bytes(s.last_freed_bytes));
                    println!("  Charging      : {}", s.is_charging);
                    println!("  Screen State  : {}", s.screen_state);
                    println!("  SoC Temp      : {:.1} °C", s.soc_temp_c);
                    println!("  Battery Temp  : {:.1} °C", s.battery_temp_c);
                    println!("==================================================");
                }
                Ok(Response::Error(err)) => {
                    eprintln!("Daemon returned error: {}", err);
                }
                Err(e) => {
                    if let Some(pid) = pid_opt {
                        let metrics = crate::system::proc_metrics::get_process_metrics_for_pid(pid);
                        println!("==================================================");
                        println!("          ANDROID CACHE CLEANER DAEMON            ");
                        println!("==================================================");
                        println!("  PID           : {}", pid);
                        println!("  State         : Active (IPC unreachable: {})", e);
                        println!("  CPU Usage     : {:.2}%", metrics.cpu_usage_pct);
                        println!("  RAM (PSS)     : {}", format_bytes(metrics.pss_bytes));
                        println!("  RAM (RSS)     : {}", format_bytes(metrics.rss_bytes));
                        println!("  RAM (Total/VM): {}", format_bytes(metrics.vm_size_bytes));
                        println!("==================================================");
                    } else {
                        println!("[!] Daemon is currently OFFLINE.");
                        println!("    Run `cleaner start` to start the daemon in the background.");
                    }
                }
                _ => {}
            }
        }

        Commands::Stats { json } => {
            match IpcClient::connect_and_send(&Command::GetStats) {
                Ok(Response::Success(ResponseData::Status(s))) => {
                    if json {
                        if let Ok(json_str) = serde_json::to_string_pretty(&s) {
                            println!("{}", json_str);
                        }
                    } else {
                        println!("Daemon Lifetime Stats:");
                        println!("  Uptime        : {} seconds", s.uptime_secs);
                        println!("  CPU Usage     : {:.2}%", s.cpu_usage_pct);
                        println!("  RAM (PSS)     : {}", format_bytes(s.ram_pss_bytes));
                        println!("  RAM (RSS)     : {}", format_bytes(s.ram_rss_bytes));
                        println!("  RAM (Total/VM): {}", format_bytes(s.ram_vm_size_bytes));
                        println!("  Total Freed   : {}", format_bytes(s.total_freed_bytes));
                    }
                }
                Err(e) => {
                    eprintln!("Failed to retrieve stats: {}", e);
                }
                _ => {}
            }
        }

        Commands::Cancel => {
            match IpcClient::connect_and_send(&Command::Cancel) {
                Ok(Response::Success(ResponseData::Message(msg))) => {
                    println!("[+] {}", msg);
                }
                Err(e) => eprintln!("Failed to cancel operation: {}", e),
                _ => {}
            }
        }

        Commands::Reload => {
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

        Commands::Info => {
            let sys = AndroidSystemInfo::detect();
            println!("==================================================");
            println!("            ANDROID PLATFORM DIAGNOSTICS          ");
            println!("==================================================");
            println!("  API Level     : {} (Android {})", sys.api_level, sys.release_version);
            println!("  Manufacturer  : {}", sys.manufacturer);
            println!("  Brand / Model : {} / {}", sys.brand, sys.model);
            println!("  Encrypted     : {}", sys.is_encrypted);
            println!("  SELinux Mode  : {:?}", get_selinux_mode());

            println!("\n[+] Discovered Android User Profiles:");
            let users = enumerate_users();
            for u in &users {
                let enc = check_encryption_state(u.user_id);
                println!(
                    "  - User ID: {} | State: {:?} | Path: {}",
                    u.user_id,
                    enc,
                    u.ce_path.display()
                );
            }

            let thermal = hardware::read_thermal();
            println!("\n[+] Hardware Diagnostics:");
            println!("  - SoC Temp     : {:.1} °C", thermal.max_soc_temp_c);
            println!("  - Battery Temp : {:.1} °C", thermal.battery_temp_c);
            println!("  - Charger State: {:?}", hardware::get_charger_state());
            println!("  - Screen State : {:?}", hardware::get_screen_state());

            let f2fs = hardware::F2fsController::discover();
            println!("  - F2FS GC Avail: {}", f2fs.is_available());
            println!("==================================================");
        }

        Commands::ConfigInit { output } => {
            let default_cfg = DaemonConfig::default();
            match default_cfg.save_to_file(&output) {
                Ok(_) => println!("[+] Generated default configuration at: {}", output.display()),
                Err(e) => eprintln!("Failed to write configuration: {}", e),
            }
        }
    }
}

fn start_daemon(config_path: Option<&PathBuf>, foreground: bool) {
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
                println!("[+] Cleaner daemon started successfully (PID: {}).", child_pid);
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

fn stop_daemon() {
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
            log::warn!("Daemon did not exit via IPC, sending SIGTERM to PID {}", pid);
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
            log::warn!("Daemon unresponsive to SIGTERM, sending SIGKILL to PID {}", pid);
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

fn print_clean_report(report: &crate::ipc::protocol::CleanReport) {
    println!("    Total Freed    : {}", format_bytes(report.total_freed_bytes));
    println!("    Files Deleted  : {}", report.deleted_files_count);
    println!("    App Cache      : {}", format_bytes(report.app_cache_freed_bytes));
    println!("    OEM Logs       : {}", format_bytes(report.oem_logs_freed_bytes));
    println!("    Crash Dumps    : {}", format_bytes(report.crash_dumps_freed_bytes));
    println!("    Temp APKs      : {}", format_bytes(report.temp_apks_freed_bytes));
    println!("    ZRAM Compacted : {}", report.zram_compacted);
    println!("    FITRIM Run     : {}", report.fstrim_completed);
    println!("    Duration       : {} ms", report.duration_ms);
}
