use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

use crate::domain::types::{ByteCount, ConfigGeneration};
use crate::error::{CleanerError, Result};

pub const PLATFORM_INVARIANT_PROTECTED_PATHS: &[&str] = &[
    "/system",
    "/vendor",
    "/product",
    "/system_ext",
    "/apex",
    "/data/system",
    "/data/misc",
    "/data/bootchart",
    "/metadata",
    "/boot",
    "/etc",
];

/// Stage 1: Raw configuration deserialized from untrusted TOML/JSON input.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawConfig {
    pub maintenance_interval_secs: Option<u64>,
    pub min_screen_off_secs: Option<u64>,
    pub max_soc_temp_c: Option<f32>,
    pub max_battery_temp_c: Option<f32>,
    pub min_app_cache_age_days: Option<u64>,
    pub min_file_age_hours: Option<u64>,
    pub app_cache_threshold_mb: Option<u64>,
    pub dry_run: Option<bool>,
    pub clean_app_cache: Option<bool>,
    pub clean_code_cache: Option<bool>,
    pub clean_tombstones: Option<bool>,
    pub clean_oem_logs: Option<bool>,
    pub clean_temp_apks: Option<bool>,
    pub whitelist_packages: Option<Vec<String>>,
    pub protected_paths: Option<Vec<String>>,
    pub fstrim_interval_hours: Option<u64>,
    pub vacuum_db_interval_hours: Option<u64>,
}

impl Default for RawConfig {
    fn default() -> Self {
        Self {
            maintenance_interval_secs: Some(3600),
            min_screen_off_secs: Some(180),
            max_soc_temp_c: Some(45.0),
            max_battery_temp_c: Some(42.0),
            min_app_cache_age_days: Some(3),
            min_file_age_hours: None,
            app_cache_threshold_mb: Some(50),
            dry_run: Some(false),
            clean_app_cache: Some(true),
            clean_code_cache: Some(false),
            clean_tombstones: Some(false),
            clean_oem_logs: Some(false),
            clean_temp_apks: Some(false),
            whitelist_packages: Some(vec![
                "com.android.vending".into(),
                "com.google.android.gms".into(),
            ]),
            protected_paths: Some(vec![
                "/system".into(),
                "/vendor".into(),
                "/apex".into(),
                "/data/data/com.android.providers.telephony".into(),
            ]),
            fstrim_interval_hours: Some(24),
            vacuum_db_interval_hours: Some(72),
        }
    }
}

/// Stage 2: Structurally validated configuration with strict invariant enforcement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatedConfig {
    pub maintenance_interval_secs: u64,
    pub min_screen_off_secs: u64,
    pub max_soc_temp_c: f32,
    pub max_battery_temp_c: f32,
    pub min_app_cache_age_secs: u64,
    pub app_cache_threshold_bytes: ByteCount,
    pub dry_run: bool,
    pub clean_app_cache: bool,
    pub clean_code_cache: bool,
    pub clean_tombstones: bool,
    pub clean_oem_logs: bool,
    pub clean_temp_apks: bool,
    pub whitelist_packages: Vec<String>,
    pub protected_paths: Vec<PathBuf>,
    pub fstrim_interval_secs: u64,
    pub vacuum_db_interval_secs: u64,
}

impl ValidatedConfig {
    pub fn from_raw(raw: RawConfig) -> Result<Self> {
        let interval = raw.maintenance_interval_secs.unwrap_or(3600);
        if !(60..=604800).contains(&interval) {
            return Err(CleanerError::ConfigError(
                "maintenance_interval_secs must be 60..604800".into(),
            ));
        }

        let min_screen_off = raw.min_screen_off_secs.unwrap_or(180);
        if min_screen_off > 3600 {
            return Err(CleanerError::ConfigError(
                "min_screen_off_secs must be 0..3600".into(),
            ));
        }
        let max_soc = raw.max_soc_temp_c.unwrap_or(45.0);

        if !(20.0..=90.0).contains(&max_soc) {
            return Err(CleanerError::ConfigError(format!(
                "max_soc_temp_c {} out of safe range [20.0, 90.0]",
                max_soc
            )));
        }

        let max_battery = raw.max_battery_temp_c.unwrap_or(45.0);
        if !(20.0..=65.0).contains(&max_battery) {
            return Err(CleanerError::ConfigError(format!(
                "max_battery_temp_c {} out of safe range [20.0, 65.0]",
                max_battery
            )));
        }
        let min_app_cache_age_secs = if let Some(hours) = raw.min_file_age_hours {
            hours.saturating_mul(3600)
        } else {
            let min_age_days = raw.min_app_cache_age_days.unwrap_or(3);
            min_age_days.saturating_mul(86400)
        };

        let threshold_mb = raw.app_cache_threshold_mb.unwrap_or(50);
        let app_cache_threshold_bytes = ByteCount::new(threshold_mb.saturating_mul(1024 * 1024));

        let mut whitelist = raw.whitelist_packages.unwrap_or_default();
        // Validate whitelist: bounded count/length, no NUL/newline, deduplicate
        if whitelist.len() > 512 {
            return Err(CleanerError::ConfigError(
                "whitelist_packages exceeds 512 entries".into(),
            ));
        }
        for pkg in &whitelist {
            if pkg.len() > 255 || pkg.contains('\0') || pkg.contains('/') || pkg.trim().is_empty() {
                return Err(CleanerError::ConfigError(format!(
                    "invalid whitelist package '{}'",
                    pkg
                )));
            }
        }
        whitelist.sort();
        whitelist.dedup();

        let mut protected_paths: Vec<PathBuf> = PLATFORM_INVARIANT_PROTECTED_PATHS
            .iter()
            .map(PathBuf::from)
            .collect();
        for user_path in raw.protected_paths.unwrap_or_default() {
            if user_path.is_empty() || user_path.len() > 4096 || user_path.contains('\0') {
                return Err(CleanerError::ConfigError(format!(
                    "invalid protected path '{}'",
                    user_path
                )));
            }
            let pb = PathBuf::from(&user_path);
            if !protected_paths.contains(&pb) {
                protected_paths.push(pb);
            }
        }

        let fstrim_hours = raw.fstrim_interval_hours.unwrap_or(24);
        if !(1..=720).contains(&fstrim_hours) {
            return Err(CleanerError::ConfigError(
                "fstrim_interval_hours must be 1..720".into(),
            ));
        }
        let vacuum_hours = raw.vacuum_db_interval_hours.unwrap_or(72);
        if !(1..=720).contains(&vacuum_hours) {
            return Err(CleanerError::ConfigError(
                "vacuum_db_interval_hours must be 1..720".into(),
            ));
        }

        Ok(Self {
            maintenance_interval_secs: interval,
            min_screen_off_secs: min_screen_off,
            max_soc_temp_c: max_soc,
            max_battery_temp_c: max_battery,
            min_app_cache_age_secs,
            app_cache_threshold_bytes,
            dry_run: raw.dry_run.unwrap_or(false),
            clean_app_cache: raw.clean_app_cache.unwrap_or(true),
            clean_code_cache: raw.clean_code_cache.unwrap_or(false),
            clean_tombstones: raw.clean_tombstones.unwrap_or(false),
            clean_oem_logs: raw.clean_oem_logs.unwrap_or(false),
            clean_temp_apks: raw.clean_temp_apks.unwrap_or(false),
            whitelist_packages: whitelist,
            protected_paths,
            fstrim_interval_secs: fstrim_hours.saturating_mul(3600),
            vacuum_db_interval_secs: vacuum_hours.saturating_mul(3600),
        })
    }

    pub fn from_toml_str(toml_str: &str) -> Result<Self> {
        let raw: RawConfig = toml::from_str(toml_str)
            .map_err(|e| CleanerError::ConfigError(format!("Failed to parse config TOML: {}", e)))?;
        Self::from_raw(raw)
    }
}

/// Stage 3: Effective configuration snapshot bound to a ConfigGeneration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectiveConfig {
    pub generation: ConfigGeneration,
    pub validated: ValidatedConfig,
}

impl EffectiveConfig {
    pub fn new(generation: ConfigGeneration, validated: ValidatedConfig) -> Self {
        Self {
            generation,
            validated,
        }
    }

    pub fn is_package_whitelisted(&self, pkg: &str) -> bool {
        self.validated
            .whitelist_packages
            .iter()
            .any(|w| w.eq_ignore_ascii_case(pkg))
    }

    pub fn is_path_protected(&self, path: &Path) -> bool {
        self.validated
            .protected_paths
            .iter()
            .any(|prot| {
                if path.starts_with(prot) {
                    return true;
                }
                let path_comps: Vec<_> = path.components().collect();
                let prot_comps: Vec<_> = prot.components().collect();
                if path_comps.len() >= prot_comps.len() && !prot_comps.is_empty() {
                    path_comps[..prot_comps.len()] == prot_comps[..]
                } else {
                    false
                }
            })
    }
}
