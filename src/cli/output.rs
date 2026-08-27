use crate::ipc::protocol::CleanReport;
use crate::util::format_bytes;

pub fn print_clean_report(report: &CleanReport) {
    println!("  [Storage Deletion]");
    println!(
        "    Total Freed    : {}",
        format_bytes(report.storage.total_freed_bytes)
    );
    println!("    Files Deleted  : {}", report.storage.deleted_files_count);
    println!(
        "    App Cache      : {}",
        format_bytes(report.storage.app_cache_bytes)
    );
    if report.storage.oem_logs_bytes > 0 {
        println!(
            "    OEM Logs       : {}",
            format_bytes(report.storage.oem_logs_bytes)
        );
    }
    if report.storage.crash_dumps_bytes > 0 {
        println!(
            "    Crash Dumps    : {}",
            format_bytes(report.storage.crash_dumps_bytes)
        );
    }
    if report.storage.temp_apks_bytes > 0 {
        println!(
            "    Temp APKs      : {}",
            format_bytes(report.storage.temp_apks_bytes)
        );
    }
    if report.storage.frozen_apps_cleaned > 0 || report.storage.active_apps_cleaned > 0 {
        println!(
            "    Frozen Apps    : {} cleaned",
            report.storage.frozen_apps_cleaned
        );
        println!(
            "    Active Apps    : {} cleaned",
            report.storage.active_apps_cleaned
        );
    }
    if report.storage.skipped_files_count > 0 {
        println!(
            "    Skipped Files  : {} (Protected / Age / Inaccessible)",
            report.storage.skipped_files_count
        );
    }
    if report.storage.errors_count > 0 {
        println!("    Errors/Denied  : {}", report.storage.errors_count);
    }

    println!("  [Memory Optimization]");
    println!("    ZRAM Compacted : {}", report.memory.zram_compacted);
    println!("    RAM Compacted  : {}", report.memory.memory_compacted);
    println!(
        "    Cgroup Reclaim : {} ({} MB)",
        report.memory.cgroup_memory_reclaimed, report.memory.reclaimed_mb
    );

    println!("  [Storage Maintenance]");
    println!("    FITRIM Status  : {}", report.trim.fstrim_completed);
    if !report.trim.trimmed_mounts.is_empty() {
        println!("    Mounts Trimmed : {}", report.trim.trimmed_mounts.join(", "));
    }
    println!("    F2FS GC Urgent : {}", report.optimization.f2fs_gc_activated);
    println!("    Job Duration   : {} ms", report.duration_ms);
}
