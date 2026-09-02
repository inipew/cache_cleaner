use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

use crate::domain::types::{ByteCount, GenerationId};
use crate::error::{CleanerError, Result};

/// Stage 1: Raw configuration deserialized from untrusted TOML/JSON input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawConfig {
    pub maintenance_interval_secs: Option<u64>,
    pub min_screen_off_secs: Option<u64>,
    pub max_soc_temp_c: Option<f32>,
    pub max_battery_temp_c: Option<f32>,
    pub min_app_cache_age_days: Option<u64>,
    pub app_cache_threshold_mb: Option<u64>,
    pub dry_run: Option<bool>,
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
            app_cache_threshold_mb: Some(50),
            dry_run: Some(false),
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
    pub whitelist_packages: Vec<String>,
    pub protected_paths: Vec<PathBuf>,
    pub fstrim_interval_secs: u64,
    pub vacuum_db_interval_secs: u64,
}

impl ValidatedConfig {
    pub fn from_raw(raw: RawConfig) -> Result<Self> {
        let interval = raw.maintenance_interval_secs.unwrap_or(3600);
        if interval < 60 {
            return Err(CleanerError::ConfigError(
                "maintenance_interval_secs must be at least 60s".into(),
            ));
        }

        let min_screen_off = raw.min_screen_off_secs.unwrap_or(180);
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
        let min_age_days = raw.min_app_cache_age_days.unwrap_or(3);
        let min_app_cache_age_secs = min_age_days.saturating_mul(86400);

        let threshold_mb = raw.app_cache_threshold_mb.unwrap_or(50);
        let app_cache_threshold_bytes = ByteCount::new(threshold_mb.saturating_mul(1024 * 1024));

        let whitelist = raw.whitelist_packages.unwrap_or_default();
        let protected_paths = raw
            .protected_paths
            .unwrap_or_default()
            .into_iter()
            .map(PathBuf::from)
            .collect();

        let fstrim_hours = raw.fstrim_interval_hours.unwrap_or(24);
        let vacuum_hours = raw.vacuum_db_interval_hours.unwrap_or(72);

        Ok(Self {
            maintenance_interval_secs: interval,
            min_screen_off_secs: min_screen_off,
            max_soc_temp_c: max_soc,
            max_battery_temp_c: max_battery,
            min_app_cache_age_secs,
            app_cache_threshold_bytes,
            dry_run: raw.dry_run.unwrap_or(false),
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

/// Stage 3: Effective configuration snapshot bound to a GenerationId.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectiveConfig {
    pub generation: GenerationId,
    pub validated: ValidatedConfig,
}

impl EffectiveConfig {
    pub fn new(generation: GenerationId, validated: ValidatedConfig) -> Self {
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
            .any(|prot| path.starts_with(prot))
    }
}
