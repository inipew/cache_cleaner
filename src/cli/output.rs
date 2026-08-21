use crate::ipc::protocol::CleanReport;
use crate::util::format_bytes;

pub fn print_clean_report(report: &CleanReport) {
    println!(
        "    Total Freed    : {}",
        format_bytes(report.total_freed_bytes)
    );
    println!("    Files Deleted  : {}", report.deleted_files_count);
    println!(
        "    App Cache      : {}",
        format_bytes(report.app_cache_freed_bytes)
    );
    println!(
        "    OEM Logs       : {}",
        format_bytes(report.oem_logs_freed_bytes)
    );
    println!(
        "    Crash Dumps    : {}",
        format_bytes(report.crash_dumps_freed_bytes)
    );
    println!(
        "    Temp APKs      : {}",
        format_bytes(report.temp_apks_freed_bytes)
    );
    if report.frozen_apps_cleaned > 0 || report.active_apps_cleaned > 0 {
        println!(
            "    Frozen Apps    : {} cleaned",
            report.frozen_apps_cleaned
        );
        println!(
            "    Active Apps    : {} cleaned",
            report.active_apps_cleaned
        );
    }
    if report.skipped_files_count > 0 {
        println!(
            "    Skipped Files  : {} (FBE locked/inaccessible)",
            report.skipped_files_count
        );
    }
    if report.errors_count > 0 {
        println!("    Errors/Denied  : {}", report.errors_count);
    }
    println!("    ZRAM Compacted : {}", report.zram_compacted);
    println!("    RAM Compacted  : {}", report.memory_compacted);
    println!("    Cgroup Reclaim : {}", report.cgroup_memory_reclaimed);
    println!("    FITRIM Run     : {}", report.fstrim_completed);
    println!("    Duration       : {} ms", report.duration_ms);
}
