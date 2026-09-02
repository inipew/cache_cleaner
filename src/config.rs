use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::Result;

// ==============================================================================
// Mandatory Fixed Paths for Android Production Deployment
// ==============================================================================
#[allow(dead_code)]
pub const BASE_DIR: &str = "/data/adb/cleaner";
#[allow(dead_code)]
pub const BIN_PATH: &str = "/data/adb/cleaner/bin/cleaner";
#[allow(dead_code)]
pub const RUN_DIR: &str = "/data/adb/cleaner/run";
#[allow(dead_code)]
pub const CONFIG_PATH: &str = "/data/adb/cleaner/config.toml";
pub const SOCKET_PATH: &str = "/data/adb/cleaner/run/daemon";
#[allow(dead_code)]
pub const LOG_PATH: &str = "/data/adb/cleaner/run/cleaner.log";
pub const PID_PATH: &str = "/data/adb/cleaner/run/cleaner.pid";
pub const LOCK_PATH: &str = "/data/adb/cleaner/run/cleaner_daemon.lock";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    #[serde(default = "default_maintenance_interval_secs")]
    pub maintenance_interval_secs: u64,

    #[serde(default = "default_socket_path")]
    pub socket_path: String,

    #[serde(default = "default_abstract_socket_name")]
    pub abstract_socket_name: String,

    #[serde(default = "default_true")]
    pub require_screen_off: bool,

    #[serde(default = "default_true")]
    pub require_charging_for_deep_clean: bool,

    #[serde(default = "default_min_screen_off_secs")]
    pub min_screen_off_secs: u64,

    #[serde(default = "default_max_soc_temp")]
    pub max_soc_temp_c: f32,

    #[serde(default = "default_max_battery_temp")]
    pub max_battery_temp_c: f32,

    #[serde(default)]
    pub cleaning: CleaningRulesConfig,

    #[serde(default)]
    pub optimization: OptimizationConfig,

    #[serde(default)]
    pub safety: SafetyConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleaningRulesConfig {
    #[serde(default = "default_true")]
    pub clean_app_cache: bool,

    #[serde(default = "default_true")]
    pub clean_webview_cache: bool,

    #[serde(default = "default_true")]
    pub clean_image_caches: bool,

    #[serde(default = "default_false")]
    pub clean_thumbnails: bool,

    #[serde(default = "default_false")]
    pub clean_code_cache: bool,

    #[serde(default = "default_false")]
    pub clean_oem_logs: bool,

    #[serde(default = "default_false")]
    pub clean_crash_dumps: bool,

    #[serde(default = "default_keep_crashes")]
    pub keep_recent_crash_files: usize,

    #[serde(default = "default_false")]
    pub clean_temp_apks: bool,

    #[serde(default = "default_min_file_age_hours")]
    pub min_file_age_hours: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationConfig {
    #[serde(default = "default_false")]
    pub zram_compaction: bool,

    #[serde(default = "default_false")]
    pub compact_memory: bool,

    #[serde(default = "default_false")]
    pub cgroup_memory_reclaim: bool,

    #[serde(default = "default_reclaim_amount_mb")]
    pub cgroup_reclaim_amount_mb: u64,

    #[serde(default = "default_true")]
    pub freezer_aware_cleaning: bool,

    #[serde(default = "default_true")]
    pub psi_adaptive_monitoring: bool,

    #[serde(default = "default_psi_moderate_stall_ms")]
    pub psi_moderate_stall_ms: u32,

    #[serde(default = "default_psi_critical_stall_ms")]
    pub psi_critical_stall_ms: u32,

    #[serde(default = "default_psi_cooldown_secs")]
    pub psi_cooldown_secs: u64,

    #[serde(default = "default_true")]
    pub f2fs_gc_urgent: bool,

    #[serde(default = "default_false")]
    pub fstrim_partitions: bool,

    #[serde(default = "default_trim_mounts")]
    pub trim_mount_points: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SafetyMode {
    Safe,       // Strict canonical app cache only, min age >= 24h, UID check
    Balanced,   // Adds WebView, Image Cache, Thumbnails
    Aggressive, // Adds OEM logs, Crash dumps, Temp APKs
}

fn default_safety_mode() -> SafetyMode {
    SafetyMode::Safe
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafetyConfig {
    #[serde(default = "default_safety_mode")]
    pub mode: SafetyMode,

    #[serde(default = "default_whitelist_packages")]
    pub whitelist_packages: Vec<String>,

    #[serde(alias = "protected_substrings", default = "default_protected_directory_names")]
    pub protected_directory_names: Vec<String>,
}

fn default_maintenance_interval_secs() -> u64 {
    3600 * 6 // 6 hours
}

fn default_socket_path() -> String {
    SOCKET_PATH.to_string()
}

fn default_abstract_socket_name() -> String {
    "cleaner_daemon".to_string()
}

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

fn default_min_screen_off_secs() -> u64 {
    180 // 3 minutes screen off before auto-clean
}

fn default_max_soc_temp() -> f32 {
    44.0 // Celsius
}

fn default_max_battery_temp() -> f32 {
    39.5 // Celsius
}

fn default_keep_crashes() -> usize {
    3
}

fn default_min_file_age_hours() -> u32 {
    24 // Default: 24 hours minimum age for cache files
}

fn default_reclaim_amount_mb() -> u64 {
    128 // 128 MB target page reclaim
}

fn default_psi_moderate_stall_ms() -> u32 {
    150 // 150ms stall in 1s window
}

fn default_psi_critical_stall_ms() -> u32 {
    250 // 250ms stall in 1s window
}

fn default_psi_cooldown_secs() -> u64 {
    45 // 45 seconds cooldown between PSI actions
}

fn default_trim_mounts() -> Vec<String> {
    vec![
        "/data".to_string(),
        "/cache".to_string(),
        "/metadata".to_string(),
    ]
}

fn default_whitelist_packages() -> Vec<String> {
    vec![
        "com.android.systemui".to_string(),
        "com.android.settings".to_string(),
        "com.android.providers.settings".to_string(),
        "com.android.providers.media".to_string(),
        "com.google.android.gms".to_string(),
        "com.google.android.googlequicksearchbox".to_string(),
    ]
}

fn default_protected_directory_names() -> Vec<String> {
    vec![
        ".nomedia".to_string(),
        "shared_prefs".to_string(),
        "databases".to_string(),
        "lib".to_string(),
        "files".to_string(),
        "keystore".to_string(),
        "fpdata".to_string(),
    ]
}

impl Default for CleaningRulesConfig {
    fn default() -> Self {
        Self {
            clean_app_cache: default_true(),
            clean_webview_cache: default_true(),
            clean_image_caches: default_true(),
            clean_thumbnails: default_false(),
            clean_code_cache: default_false(),
            clean_oem_logs: default_false(),
            clean_crash_dumps: default_false(),
            keep_recent_crash_files: default_keep_crashes(),
            clean_temp_apks: default_false(),
            min_file_age_hours: default_min_file_age_hours(),
        }
    }
}

impl Default for OptimizationConfig {
    fn default() -> Self {
        Self {
            zram_compaction: default_false(),
            compact_memory: default_false(),
            cgroup_memory_reclaim: default_false(),
            cgroup_reclaim_amount_mb: default_reclaim_amount_mb(),
            freezer_aware_cleaning: default_true(),
            psi_adaptive_monitoring: default_true(),
            psi_moderate_stall_ms: default_psi_moderate_stall_ms(),
            psi_critical_stall_ms: default_psi_critical_stall_ms(),
            psi_cooldown_secs: default_psi_cooldown_secs(),
            f2fs_gc_urgent: default_true(),
            fstrim_partitions: default_false(),
            trim_mount_points: default_trim_mounts(),
        }
    }
}

impl Default for SafetyConfig {
    fn default() -> Self {
        Self {
            mode: default_safety_mode(),
            whitelist_packages: default_whitelist_packages(),
            protected_directory_names: default_protected_directory_names(),
        }
    }
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            maintenance_interval_secs: default_maintenance_interval_secs(),
            socket_path: default_socket_path(),
            abstract_socket_name: default_abstract_socket_name(),
            require_screen_off: default_true(),
            require_charging_for_deep_clean: default_true(),
            min_screen_off_secs: default_min_screen_off_secs(),
            max_soc_temp_c: default_max_soc_temp(),
            max_battery_temp_c: default_max_battery_temp(),
            cleaning: CleaningRulesConfig::default(),
            optimization: OptimizationConfig::default(),
            safety: SafetyConfig::default(),
        }
    }
}

impl DaemonConfig {
    /// Validates configuration values against safe runtime bounds.
    pub fn validate(&self) -> Result<()> {
        if self.maintenance_interval_secs < 30 {
            return Err(crate::error::CleanerError::Config(
                "maintenance_interval_secs must be at least 30 seconds".to_string(),
            ));
        }

        if self.max_soc_temp_c < 20.0 || self.max_soc_temp_c > 95.0 {
            return Err(crate::error::CleanerError::Config(
                "max_soc_temp_c must be between 20.0°C and 95.0°C".to_string(),
            ));
        }

        if self.max_battery_temp_c < 20.0 || self.max_battery_temp_c > 65.0 {
            return Err(crate::error::CleanerError::Config(
                "max_battery_temp_c must be between 20.0°C and 65.0°C".to_string(),
            ));
        }

        if self.safety.protected_directory_names.is_empty() {
            return Err(crate::error::CleanerError::Config(
                "safety.protected_directory_names cannot be empty".to_string(),
            ));
        }

        Ok(())
    }

    /// Loads and validates a configuration file strictly, returning an error if parsing or validation fails.
    pub fn load_from_file_strict<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = fs::read_to_string(path.as_ref())?;
        let config: DaemonConfig = toml::from_str(&content)?;
        config.validate()?;
        log::info!(
            "Loaded configuration strictly from {}",
            path.as_ref().display()
        );
        Ok(config)
    }

    /// Loads configuration and returns the active path.
    /// Strictly prioritizes /data/adb/cleaner/config.toml.
    pub fn load_or_default_with_path<P: AsRef<Path>>(path: Option<P>) -> (Self, Option<PathBuf>) {
        if let Some(p) = path {
            let path_ref = p.as_ref();
            match Self::load_from_file_strict(path_ref) {
                Ok(cfg) => {
                    return (cfg, Some(path_ref.to_path_buf()));
                }
                Err(e) => {
                    log::warn!(
                        "Failed to strictly load config at {}: {}.",
                        path_ref.display(),
                        e
                    );
                }
            }
        }

        // Primary mandatory path: /data/adb/cleaner/config.toml
        let mandatory_path = PathBuf::from(CONFIG_PATH);
        if mandatory_path.exists() {
            if let Ok(cfg) = Self::load_from_file_strict(&mandatory_path) {
                return (cfg, Some(mandatory_path));
            }
        }

        // Local development fallback: config.toml in current directory
        let local_path = PathBuf::from("config.toml");
        if local_path.exists() {
            if let Ok(cfg) = Self::load_from_file_strict(&local_path) {
                return (cfg, Some(local_path));
            }
        }

        log::info!("Using optimized builtin configuration defaults");
        (DaemonConfig::default(), None)
    }

    #[allow(dead_code)]
    pub fn load_or_default<P: AsRef<Path>>(path: Option<P>) -> Self {
        let (config, _) = Self::load_or_default_with_path(path);
        config
    }

    pub fn reload_from_path<P: AsRef<Path>>(path: Option<P>) -> Result<Self> {
        if let Some(p) = path {
            Self::load_from_file_strict(p)
        } else {
            let mandatory = Path::new(CONFIG_PATH);
            if mandatory.exists() {
                Self::load_from_file_strict(mandatory)
            } else {
                let (cfg, _) = Self::load_or_default_with_path(None::<&str>);
                Ok(cfg)
            }
        }
    }

    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        use std::io::Write;

        self.validate()?;
        let content = toml::to_string_pretty(self)?;
        let target = path.as_ref();
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp_path = target.with_extension("tmp");
        {
            let mut file = fs::File::create(&tmp_path)?;
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
        }
        fs::rename(&tmp_path, target)?;
        Ok(())
    }
}
