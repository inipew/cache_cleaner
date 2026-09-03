use std::path::Path;

use crate::config::DaemonConfig;
use crate::hardware;
use crate::ipc::{Command, IpcClient, Response, ResponseData};
use crate::platform::{
    check_encryption_state, enumerate_users, get_selinux_mode, AndroidSystemInfo,
};
use crate::system::get_running_pid;
use crate::util::format_bytes;

pub fn show_status() {
    let pid_opt = get_running_pid();

    match IpcClient::connect_and_send(&Command::GetStatus) {
        Ok(Response::Success(ResponseData::Status(s))) => {
            println!("==================================================");
            println!("          ANDROID CACHE CLEANER DAEMON            ");
            println!("==================================================");
            println!(
                "  PID           : {}",
                pid_opt
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "N/A".to_string())
            );
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

pub fn show_stats(json: bool) {
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

pub fn show_platform_info() {
    let sys = AndroidSystemInfo::detect();
    println!("==================================================");
    println!("            ANDROID PLATFORM DIAGNOSTICS          ");
    println!("==================================================");
    println!(
        "  API Level     : {} (Android {})",
        sys.api_level, sys.release_version
    );
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

    let cgroup_diag = crate::system::cgroup::get_cgroup_diagnostics();
    println!("\n[+] Cgroups & Kernel Resource Isolation:");
    println!("  - Hierarchy    : {}", cgroup_diag.detected_version);
    println!(
        "  - Mem Reclaim  : {}",
        if cgroup_diag.supports_memory_reclaim {
            "Supported (Cgroup v2)"
        } else {
            "Not Available / v1"
        }
    );
    if !cgroup_diag.controllers_available.is_empty() {
        println!(
            "  - Controllers  : {}",
            cgroup_diag.controllers_available.join(", ")
        );
    }
    if !cgroup_diag.current_process_cgroups.is_empty() {
        println!(
            "  - Current Group: {}",
            cgroup_diag
                .current_process_cgroups
                .first()
                .unwrap_or(&"N/A".to_string())
        );
    }

    let freezer_diag = crate::system::freezer::get_freezer_diagnostics();
    println!("\n[+] Android App Freezer (Cgroup Freezer):");
    println!(
        "  - App Freezer  : {}",
        if freezer_diag.is_cached_apps_freezer_enabled {
            "Enabled (Active)"
        } else {
            "Disabled / Inactive"
        }
    );
    println!("  - Hierarchy    : {}", freezer_diag.freezer_cgroup_version);
    println!(
        "  - Frozen UIDs  : {} processes currently frozen in RAM",
        freezer_diag.total_frozen_uids_count
    );

    let psi_diag = crate::system::psi::get_psi_diagnostics();
    println!("\n[+] Kernel Pressure Stall Information (PSI):");
    println!(
        "  - PSI Supported: {}",
        if psi_diag.is_supported {
            "Yes (/proc/pressure active)"
        } else {
            "No / Kernel PSI disabled"
        }
    );
    println!("  - Pressure Lvl : {}", psi_diag.current_level);
    if let Some(ref mem) = psi_diag.memory_metrics {
        let full_avg = mem
            .full
            .map(|f| format!("{:.2}%", f.avg10))
            .unwrap_or_else(|| "N/A".to_string());
        println!(
            "  - Mem Pressure : some avg10={:.2}%, full avg10={}",
            mem.some.avg10, full_avg
        );
    }
    if let Some(ref io) = psi_diag.io_metrics {
        let full_avg = io
            .full
            .map(|f| format!("{:.2}%", f.avg10))
            .unwrap_or_else(|| "N/A".to_string());
        println!(
            "  - I/O Pressure : some avg10={:.2}%, full avg10={}",
            io.some.avg10, full_avg
        );
    }
    println!("==================================================");
}

pub fn init_config(output: &Path) {
    let default_cfg = DaemonConfig::default();
    match default_cfg.save_to_file(output) {
        Ok(_) => println!(
            "[+] Generated default configuration at: {}",
            output.display()
        ),
        Err(e) => eprintln!("Failed to write configuration: {}", e),
    }
}

pub fn explain_path(config: &DaemonConfig, target_path: &Path) {
    use crate::config_pipeline::{EffectiveConfig, ValidatedConfig};
    use crate::domain::candidate::Candidate;
    use crate::domain::decision::PolicyDecision;
    use crate::domain::target::{TargetClass, TargetDescriptor, TargetSafetyTier};
    use crate::domain::types::{CatalogGeneration, ConfigGeneration,
        ByteCount, CandidateId, DeviceNumber, FileIdentity, InodeNumber,
        RelativePath, TargetId, UnixTimestamp,
    };
    use crate::policy::PolicyEngine;
    use crate::safety::SafetyGate;

    println!("==================================================");
    println!("              PATH DECISION AUDIT                 ");
    println!("==================================================");
    println!("  Target Path   : {}", target_path.display());
    println!("  Exists on Disk: {}", target_path.exists());

    let meta = target_path.symlink_metadata().ok();
    if let Some(ref m) = meta {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            println!("  Device / Inode: {} / {}", m.dev(), m.ino());
            println!("  UID / GID     : {} / {}", m.uid(), m.gid());
            println!("  Is Symlink    : {}", m.file_type().is_symlink());
            println!("  File Size     : {}", format_bytes(m.len()));
        }
    }

    let val_cfg = ValidatedConfig {
        maintenance_interval_secs: config.maintenance_interval_secs,
        min_screen_off_secs: config.min_screen_off_secs,
        max_soc_temp_c: config.max_soc_temp_c,
        max_battery_temp_c: config.max_battery_temp_c,
        min_app_cache_age_secs: (config.cleaning.min_file_age_hours as u64).saturating_mul(3600),
        app_cache_threshold_bytes: ByteCount::new(50 * 1024 * 1024),
        dry_run: false,
        clean_app_cache: config.cleaning.clean_app_cache,
        clean_code_cache: config.cleaning.clean_code_cache,
        clean_tombstones: config.cleaning.clean_crash_dumps,
        clean_oem_logs: config.cleaning.clean_oem_logs,
        clean_temp_apks: config.cleaning.clean_temp_apks,
        whitelist_packages: config.safety.whitelist_packages.clone(),
        protected_paths: config
            .safety
            .protected_directory_names
            .iter()
            .map(std::path::PathBuf::from)
            .collect(),
        fstrim_interval_secs: 86400,
        vacuum_db_interval_secs: 72 * 3600,
    };
    let eff_cfg = EffectiveConfig::new(ConfigGeneration::INITIAL, val_cfg);

    let dev = meta.as_ref().map(|m| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            m.dev()
        }
        #[cfg(not(unix))]
        {
            0
        }
    }).unwrap_or(0);

    let ino = meta.as_ref().map(|m| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            m.ino()
        }
        #[cfg(not(unix))]
        {
            0
        }
    }).unwrap_or(0);

    let parent_dir = target_path.parent().unwrap_or(target_path);
    let file_name = target_path.file_name().and_then(|n| n.to_str()).unwrap_or("target");

    let descriptor = TargetDescriptor {
        target_id: TargetId::new("manual:audit:target"),
        target_class: TargetClass::AppCache,
        base_path: parent_dir.to_path_buf(),
        dev: DeviceNumber(dev),
        ino: InodeNumber(ino),
        owner_uid: 0,
        owner_gid: 0,
        package_name: None,
        safety_tier: TargetSafetyTier::StandardCache,
        catalog_generation: CatalogGeneration::INITIAL,
    };

    let candidate = Candidate {
        candidate_id: CandidateId(1),
        target_id: descriptor.target_id.clone(),
        rel_path: RelativePath::parse(file_name).unwrap_or_else(RelativePath::empty),
        identity: FileIdentity::new(dev, ino),
        size_bytes: ByteCount::new(meta.as_ref().map(|m| m.len()).unwrap_or(0)),
        mtime: UnixTimestamp::now(),
        atime: None,
        is_dir: meta.as_ref().map(|m| m.is_dir()).unwrap_or(false),
        is_symlink: meta.as_ref().map(|m| m.file_type().is_symlink()).unwrap_or(false),
    };

    let safety = SafetyGate::new();
    let policy = PolicyEngine::new();

    println!("--------------------------------------------------");
    match safety.validate_candidate(candidate, &descriptor) {
        Ok(validated) => {
            let decision = policy.evaluate_candidate(validated, &descriptor, &eff_cfg, UnixTimestamp::now());
            match decision {
                PolicyDecision::Allow(permit) => {
                    println!("  Action         : ALLOW (Eligible for deletion)");
                    println!("  Decision Reason: {}", permit.reason);
                }
                PolicyDecision::Deny(deny) => {
                    println!("  Action         : DENY (Protected / Ignored)");
                    println!("  Decision Reason: {}", deny.reason);
                }
                PolicyDecision::Skip(skip) => {
                    println!("  Action         : SKIP");
                    println!("  Decision Reason: {}", skip.reason);
                }
            }
        }
        Err(e) => {
            println!("  Action         : REJECTED BY SAFETY GATE");
            println!("  Safety Error   : {}", e);
        }
    }
    println!("==================================================");
}

pub fn show_idle_assessment(explain: bool, json: bool) {
    let response = IpcClient::connect_and_send(&Command::GetIdleAssessment);
    match response {
        Ok(Response::Success(ResponseData::Idle(assessment))) => {
            if json {
                if let Ok(json_str) = serde_json::to_string_pretty(&assessment) {
                    println!("{}", json_str);
                }
            } else {
                println!("==================================================");
                println!("            ADAPTIVE IDLE ASSESSMENT              ");
                println!("==================================================");
                println!("  Idle State    : {}", assessment.state);
                println!("  Idle Score    : {} / 100", assessment.score);
                println!("  Thermal State : {:?}", assessment.thermal_state);
                println!("  Standard Clean: {}", assessment.standard_maintenance);
                println!("  Heavy Clean   : {}", assessment.heavy_maintenance);
                println!("  Work Rate     : {} ops/sec", assessment.rate_limit_ops_per_sec);
                if let Some(next) = assessment.time_until_next_transition {
                    println!("  Next Transition: in {}s", next.as_secs());
                }

                if explain {
                    println!("--------------------------------------------------");
                    println!("  Positive Factors:");
                    for pos in &assessment.positives {
                        println!("    • {}", pos.description());
                    }
                    if !assessment.blockers.is_empty() {
                        println!("  Active Blockers:");
                        for b in &assessment.blockers {
                            println!("    ✗ {}", b.description());
                        }
                    }
                }
                println!("==================================================");
            }
        }
        Ok(Response::Error(err)) => {
            eprintln!("Daemon returned error: {}", err);
        }
        Err(e) => {
            eprintln!("Daemon is unreachable via IPC ({})", e);
        }
        _ => {}
    }
}

