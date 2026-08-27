use std::path::Path;

use crate::config::{CleaningRulesConfig, SafetyConfig, SafetyMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JunkCategory {
    AppCache,
    WebViewCache,
    ImageCache,
    Thumbnail,
    CodeCache,
    OemLog,
    CrashDump,
    TempApk,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    ProtectedDirectory(String),
    WhitelistedPackage(String),
    CodeCacheProtected,
    DisabledByConfig(&'static str),
    NotRecognizedAsJunk,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Delete {
        category: JunkCategory,
        reason: &'static str,
    },
    Skip {
        reason: SkipReason,
    },
}

impl Decision {
    #[inline]
    pub fn is_delete(&self) -> bool {
        matches!(self, Decision::Delete { .. })
    }

    #[inline]
    pub fn category(&self) -> Option<JunkCategory> {
        match self {
            Decision::Delete { category, .. } => Some(*category),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JunkType {
    AppCache,
    WebViewCache,
    ImageCache,
    Thumbnail,
    CodeCache,
    OemLog,
    CrashDump,
    TempApk,
    Ignored,
}

impl From<Decision> for JunkType {
    fn from(decision: Decision) -> Self {
        match decision {
            Decision::Delete { category, .. } => match category {
                JunkCategory::AppCache => JunkType::AppCache,
                JunkCategory::WebViewCache => JunkType::WebViewCache,
                JunkCategory::ImageCache => JunkType::ImageCache,
                JunkCategory::Thumbnail => JunkType::Thumbnail,
                JunkCategory::CodeCache => JunkType::CodeCache,
                JunkCategory::OemLog => JunkType::OemLog,
                JunkCategory::CrashDump => JunkType::CrashDump,
                JunkCategory::TempApk => JunkType::TempApk,
            },
            Decision::Skip { .. } => JunkType::Ignored,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackagePolicy {
    pub package: String,
    pub allow_cache: bool,
    pub allow_external_cache: bool,
}

/// Validates Android package name format: ^[a-zA-Z][a-zA-Z0-9_]*(\.[a-zA-Z0-9_]+)+$
pub fn is_valid_package_name(name: &str) -> bool {
    let mut parts = name.split('.');
    let first = match parts.next() {
        Some(f) if !f.is_empty() => f,
        _ => return false,
    };

    let mut first_chars = first.chars();
    if !first_chars.next().map_or(false, |c| c.is_ascii_alphabetic()) {
        return false;
    }
    if !first_chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return false;
    }

    let mut count = 1;
    for part in parts {
        if part.is_empty() {
            return false;
        }
        let mut chars = part.chars();
        if !chars.next().map_or(false, |c| c.is_ascii_alphanumeric() || c == '_') {
            return false;
        }
        if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return false;
        }
        count += 1;
    }
    count >= 2
}

pub struct RuleEngine {
    rules: CleaningRulesConfig,
    safety: SafetyConfig,
}

const OEM_LOG_ROOTS: &[&str] = &[
    "/data/miui",
    "/data/mqsas",
    "/data/system/theme_magic",
    "/data/log",
    "/data/sec_log",
    "/data/slog",
    "/data/oppo/log",
    "/data/oplus/log",
    "/data/vendor/oppo/log",
    "/data/vivo-apps/cache",
    "/data/vendor/mtklog",
    "/data/vendor/qcom",
    "/data/vendor/ramdump",
    "/data/vendor/connsys",
    "/data/log/hilog",
];

const CRASH_DUMP_ROOTS: &[&str] = &["/data/tombstones", "/data/anr", "/data/system/dropbox"];

const STAGED_APK_ROOTS: &[&str] = &["/data/app-staging", "/data/system/package_cache"];

impl RuleEngine {
    pub fn new(rules: CleaningRulesConfig, safety: SafetyConfig) -> Self {
        Self { rules, safety }
    }

    pub fn rules(&self) -> &CleaningRulesConfig {
        &self.rules
    }

    pub fn safety(&self) -> &SafetyConfig {
        &self.safety
    }

    /// Full policy evaluation producing a typed Decision (Delete or Skip with reason)
    pub fn evaluate_path(&self, path: &Path) -> Decision {
        let path_str = path.to_string_lossy();

        // 1. Absolute Deny: System / Immutable partition protection
        if path.starts_with("/system")
            || path.starts_with("/vendor")
            || path.starts_with("/apex")
            || path.starts_with("/product")
            || path.starts_with("/system_ext")
            || path.starts_with("/etc")
            || path.starts_with("/proc")
            || path.starts_with("/sys")
            || path.starts_with("/dev")
        {
            return Decision::Skip {
                reason: SkipReason::ProtectedDirectory("system_immutable_partition".to_string()),
            };
        }

        // 2. Absolute Deny: Safety Protected Directory component match
        // E.g. "databases", "shared_prefs", "lib", "files", "keystore", "fpdata", ".nomedia"
        for protected in &self.safety.protected_directory_names {
            if path
                .components()
                .any(|c| c.as_os_str() == protected.as_str())
            {
                return Decision::Skip {
                    reason: SkipReason::ProtectedDirectory(protected.clone()),
                };
            }
        }

        // 3. Absolute Deny: Whitelisted packages are 100% immune from any deletion
        for pkg in &self.safety.whitelist_packages {
            let matches_pkg = path.components().any(|c| {
                let s = c.as_os_str().to_string_lossy();
                s == *pkg || s.strip_prefix(pkg.as_str()).is_some_and(|rest| rest.starts_with('.'))
            });

            if matches_pkg {
                return Decision::Skip {
                    reason: SkipReason::WhitelistedPackage(pkg.clone()),
                };
            }
        }

        // 2. Strict JIT / ART Code Cache Protection (NEVER in Safe or Balanced modes)
        let is_jit_or_art = path.components().any(|c| {
            let s = c.as_os_str().to_string_lossy();
            s == "code_cache" || s == "oat" || s == "dalvik-cache"
        }) || path_str.ends_with(".prof")
            || path_str.ends_with(".cur.prof")
            || path_str.ends_with(".profm")
            || path_str.ends_with(".art")
            || path_str.ends_with(".odex")
            || path_str.ends_with(".vdex");

        if is_jit_or_art {
            if self.safety.mode == SafetyMode::Aggressive && self.rules.clean_code_cache {
                return Decision::Delete {
                    category: JunkCategory::CodeCache,
                    reason: "ART/JIT code cache enabled in config under aggressive safety mode",
                };
            } else {
                return Decision::Skip {
                    reason: SkipReason::CodeCacheProtected,
                };
            }
        }

        // 3. Classify by path components and patterns

        // A. WebView / Chrome cache
        let is_webview_cache = path_str.contains("app_webview/Default/Cache")
            || path_str.contains("app_webview/Default/Code Cache")
            || path_str.contains("app_webview/Default/GPUCache")
            || path_str.contains("app_webview/Default/Service Worker/CacheStorage")
            || path_str.contains("app_webview/Default/Service Worker/ScriptCache")
            || path_str.contains("org.chromium.android_webview")
            || path_str.contains("app_textures")
            || path_str.contains("splash_cache");

        if is_webview_cache {
            if self.rules.clean_webview_cache {
                return Decision::Delete {
                    category: JunkCategory::WebViewCache,
                    reason: "WebView/Chromium cache artifact",
                };
            } else {
                return Decision::Skip {
                    reason: SkipReason::DisabledByConfig("clean_webview_cache"),
                };
            }
        }

        // B. Common image & network cache libraries
        let is_image_or_net_cache = path.components().any(|c| {
            let s = c.as_os_str().to_string_lossy();
            s == "image_cache"
                || s == "fresco_cache"
                || s == "glide_cache"
                || s == "coil_cache"
                || s == "http-engine-cache"
                || s == "picasso-cache"
                || s == "volley"
                || s == "disk_cache"
                || s == "network_cache"
        });

        if is_image_or_net_cache {
            if self.rules.clean_image_caches {
                return Decision::Delete {
                    category: JunkCategory::ImageCache,
                    reason: "Application image or network cache library folder",
                };
            } else {
                return Decision::Skip {
                    reason: SkipReason::DisabledByConfig("clean_image_caches"),
                };
            }
        }

        // C. App internal/external cache folder
        let is_in_cache_folder = path.components().any(|c| {
            let s = c.as_os_str().to_string_lossy();
            s == "cache" || s == "app_cache"
        });

        if is_in_cache_folder {
            if self.rules.clean_app_cache {
                return Decision::Delete {
                    category: JunkCategory::AppCache,
                    reason: "Standard Android application cache directory",
                };
            } else {
                return Decision::Skip {
                    reason: SkipReason::DisabledByConfig("clean_app_cache"),
                };
            }
        }

        // Categories below require at least Balanced or Aggressive SafetyMode

        // D. Thumbnail caches (Balanced or Aggressive only)
        let is_thumbnail = path.components().any(|c| {
            let s = c.as_os_str().to_string_lossy();
            s == ".thumbnails"
                || s == ".thumb"
                || s == ".thumbcache"
                || s == ".video_thumbnails"
                || s == ".albumthumbs"
                || s == "micro_thumbnail"
        });

        if is_thumbnail {
            if self.safety.mode == SafetyMode::Safe {
                return Decision::Skip {
                    reason: SkipReason::DisabledByConfig("thumbnails_disabled_in_safe_mode"),
                };
            }
            if self.rules.clean_thumbnails {
                return Decision::Delete {
                    category: JunkCategory::Thumbnail,
                    reason: "Media thumbnail cache folder",
                };
            } else {
                return Decision::Skip {
                    reason: SkipReason::DisabledByConfig("clean_thumbnails"),
                };
            }
        }

        // Categories below require Aggressive SafetyMode (OEM logs, crash dumps, temp APKs)

        // E. OEM Vendor Logs (Aggressive only)
        let is_oem_log_root = OEM_LOG_ROOTS.iter().any(|root| path.starts_with(root));
        if is_oem_log_root {
            if self.safety.mode != SafetyMode::Aggressive {
                return Decision::Skip {
                    reason: SkipReason::DisabledByConfig("oem_logs_require_aggressive_mode"),
                };
            }
            if self.rules.clean_oem_logs {
                // Safe OEM log pattern filtering
                let is_log_file = path_str.ends_with(".log")
                    || path_str.ends_with(".txt")
                    || path_str.ends_with(".old")
                    || path_str.ends_with(".trace")
                    || path_str.ends_with(".dump")
                    || path_str.ends_with(".gz")
                    || path_str.contains("/log/")
                    || path_str.contains("/logs/")
                    || path_str.contains("/mobilelog/")
                    || path_str.contains("/netlog/")
                    || path_str.contains("/connsyslog/");

                if is_log_file || !path.is_file() {
                    return Decision::Delete {
                        category: JunkCategory::OemLog,
                        reason: "OEM vendor diagnostic/debug log",
                    };
                }
            } else {
                return Decision::Skip {
                    reason: SkipReason::DisabledByConfig("clean_oem_logs"),
                };
            }
        }

        // F. Crash Dumps, ANR, and DropBox (Aggressive only)
        let is_crash_dump_root = CRASH_DUMP_ROOTS.iter().any(|root| path.starts_with(root));
        if is_crash_dump_root {
            if self.safety.mode != SafetyMode::Aggressive {
                return Decision::Skip {
                    reason: SkipReason::DisabledByConfig("crash_dumps_require_aggressive_mode"),
                };
            }
            if self.rules.clean_crash_dumps {
                return Decision::Delete {
                    category: JunkCategory::CrashDump,
                    reason: "System crash dump or ANR trace",
                };
            } else {
                return Decision::Skip {
                    reason: SkipReason::DisabledByConfig("clean_crash_dumps"),
                };
            }
        }

        // G. Temporary APKs and Split Dex files (Aggressive only)
        let is_temp_apk_root = STAGED_APK_ROOTS.iter().any(|root| path.starts_with(root));
        if is_temp_apk_root {
            if self.safety.mode != SafetyMode::Aggressive {
                return Decision::Skip {
                    reason: SkipReason::DisabledByConfig("temp_apks_require_aggressive_mode"),
                };
            }
            if self.rules.clean_temp_apks {
                let is_apk_artifact = path_str.ends_with(".apk")
                    || path_str.ends_with(".dex")
                    || path_str.ends_with(".tmp")
                    || path_str.ends_with(".apk.tmp");

                if is_apk_artifact || !path.is_file() {
                    return Decision::Delete {
                        category: JunkCategory::TempApk,
                        reason: "Temporary APK installation session artifact",
                    };
                }
            } else {
                return Decision::Skip {
                    reason: SkipReason::DisabledByConfig("clean_temp_apks"),
                };
            }
        } else if path.starts_with("/data/local/tmp") {
            if self.safety.mode != SafetyMode::Aggressive {
                return Decision::Skip {
                    reason: SkipReason::DisabledByConfig("temp_apks_require_aggressive_mode"),
                };
            }
            let is_temp_artifact = path_str.ends_with(".apk")
                || path_str.ends_with(".tmp")
                || path_str.ends_with(".apks")
                || path_str.ends_with(".xapk")
                || path_str.ends_with(".dex");
            if is_temp_artifact {
                if self.rules.clean_temp_apks {
                    return Decision::Delete {
                        category: JunkCategory::TempApk,
                        reason: "Temporary APK or DEX artifact in /data/local/tmp",
                    };
                } else {
                    return Decision::Skip {
                        reason: SkipReason::DisabledByConfig("clean_temp_apks"),
                    };
                }
            }
        }

        Decision::Skip {
            reason: SkipReason::NotRecognizedAsJunk,
        }
    }

    /// Determines whether a given path is safe to delete and what kind of junk it is.
    pub fn classify_path(&self, path: &Path) -> JunkType {
        self.evaluate_path(path).into()
    }

    pub fn get_system_junk_targets(&self) -> Vec<&'static str> {
        let mut targets = Vec::new();
        if self.rules.clean_oem_logs {
            targets.extend_from_slice(OEM_LOG_ROOTS);
        }
        if self.rules.clean_crash_dumps {
            targets.extend_from_slice(CRASH_DUMP_ROOTS);
        }
        if self.rules.clean_temp_apks {
            targets.extend_from_slice(STAGED_APK_ROOTS);
            targets.push("/data/local/tmp");
        }
        targets
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_package_name_validation() {
        assert!(is_valid_package_name("com.whatsapp"));
        assert!(is_valid_package_name("com.android.chrome"));
        assert!(is_valid_package_name("org.chromium.android_webview"));
        assert!(is_valid_package_name("com.example.app_123"));

        assert!(!is_valid_package_name("invalid"));
        assert!(!is_valid_package_name("123.com.test"));
        assert!(!is_valid_package_name("com.test..app"));
        assert!(!is_valid_package_name(""));
    }

    #[test]
    fn test_jit_code_cache_is_strictly_protected_by_default() {
        let rules = CleaningRulesConfig {
            clean_app_cache: true,
            clean_code_cache: false,
            ..Default::default()
        };
        let safety = SafetyConfig::default();
        let engine = RuleEngine::new(rules, safety);

        assert_eq!(
            engine.classify_path(Path::new("/data/data/com.android.chrome/code_cache")),
            JunkType::Ignored
        );
        assert_eq!(
            engine.classify_path(Path::new(
                "/data/user/0/com.whatsapp/code_cache/compiled_view.dex"
            )),
            JunkType::Ignored
        );
        assert_eq!(
            engine.classify_path(Path::new(
                "/data/data/com.spotify.music/code_cache/oat/arm64/base.odex"
            )),
            JunkType::Ignored
        );
        assert_eq!(
            engine.classify_path(Path::new(
                "/data/misc/profiles/cur/0/com.instagram.android/primary.prof"
            )),
            JunkType::Ignored
        );
        assert_eq!(
            engine.classify_path(Path::new(
                "/data/dalvik-cache/arm64/system@framework@boot.art"
            )),
            JunkType::Ignored
        );
    }

    #[test]
    fn test_app_cache_is_correctly_classified() {
        let rules = CleaningRulesConfig {
            clean_app_cache: true,
            clean_code_cache: false,
            ..Default::default()
        };
        let safety = SafetyConfig::default();
        let engine = RuleEngine::new(rules, safety);

        assert_eq!(
            engine.classify_path(Path::new("/data/data/com.whatsapp/cache")),
            JunkType::AppCache
        );
        assert_eq!(
            engine.classify_path(Path::new("/data/data/com.whatsapp/cache/image_01.jpg")),
            JunkType::AppCache
        );
        assert_eq!(
            engine.classify_path(Path::new(
                "/data/media/0/Android/data/com.spotify.music/cache"
            )),
            JunkType::AppCache
        );
    }

    #[test]
    fn test_package_name_containing_cache_word_is_not_false_positive() {
        let rules = CleaningRulesConfig {
            clean_app_cache: true,
            ..Default::default()
        };
        let safety = SafetyConfig::default();
        let engine = RuleEngine::new(rules, safety);

        assert_eq!(
            engine.classify_path(Path::new(
                "/data/data/com.geocache.navigator/databases/geocache.db"
            )),
            JunkType::Ignored
        );
        assert_eq!(
            engine.classify_path(Path::new(
                "/data/data/com.geocache.navigator/files/userdata.json"
            )),
            JunkType::Ignored
        );
        assert_eq!(
            engine.classify_path(Path::new(
                "/data/data/com.geocache.navigator/cache/map_tile_12.png"
            )),
            JunkType::AppCache
        );
    }

    #[test]
    fn test_app_data_subfolder_with_oem_names_is_not_false_positive() {
        let rules = CleaningRulesConfig {
            clean_oem_logs: true,
            ..Default::default()
        };
        let safety = SafetyConfig {
            mode: SafetyMode::Aggressive,
            ..Default::default()
        };
        let engine = RuleEngine::new(rules, safety);

        assert_eq!(
            engine.classify_path(Path::new(
                "/data/data/com.example.miui/files/custom_log.txt"
            )),
            JunkType::Ignored
        );
        assert_eq!(
            engine.classify_path(Path::new(
                "/data/user/0/com.samsung.custom/files/sec_log/app_data.bin"
            )),
            JunkType::Ignored
        );
        assert_eq!(
            engine.classify_path(Path::new("/data/miui/debug_log.txt")),
            JunkType::OemLog
        );
        assert_eq!(
            engine.classify_path(Path::new("/data/log/kernel_log.log")),
            JunkType::OemLog
        );
    }

    #[test]
    fn test_protected_directory_names_cannot_be_deleted() {
        let rules = CleaningRulesConfig::default();
        let safety = SafetyConfig::default();
        let engine = RuleEngine::new(rules, safety);

        assert_eq!(
            engine.classify_path(Path::new("/data/data/com.whatsapp/databases/msgstore.db")),
            JunkType::Ignored
        );
        assert_eq!(
            engine.classify_path(Path::new("/data/data/com.whatsapp/shared_prefs/prefs.xml")),
            JunkType::Ignored
        );
        assert_eq!(
            engine.classify_path(Path::new("/data/data/com.whatsapp/files/key_store")),
            JunkType::Ignored
        );
        assert_eq!(
            engine.classify_path(Path::new("/data/data/com.whatsapp/lib/libtest.so")),
            JunkType::Ignored
        );
        assert_eq!(
            engine.classify_path(Path::new("/data/media/0/Pictures/.nomedia")),
            JunkType::Ignored
        );

        assert_eq!(
            engine.classify_path(Path::new("/data/data/com.libra.browser/cache/cached_image.png")),
            JunkType::AppCache
        );
        assert_eq!(
            engine.classify_path(Path::new("/data/data/com.delivery.profiles/cache/tile.png")),
            JunkType::AppCache
        );
        assert_eq!(
            engine.classify_path(Path::new("/data/data/com.database.explorer/cache/query_cache.bin")),
            JunkType::AppCache
        );

        assert_eq!(
            engine.classify_path(Path::new("/data/data/com.libra.browser/databases/bookmarks.db")),
            JunkType::Ignored
        );
        assert_eq!(
            engine.classify_path(Path::new("/data/data/com.libra.browser/lib/libengine.so")),
            JunkType::Ignored
        );
        assert_eq!(
            engine.classify_path(Path::new("/data/data/com.delivery.profiles/files/user.json")),
            JunkType::Ignored
        );
    }

    #[test]
    fn test_local_tmp_safety_rules() {
        let rules = CleaningRulesConfig {
            clean_temp_apks: true,
            ..Default::default()
        };
        let safety = SafetyConfig {
            mode: SafetyMode::Aggressive,
            ..Default::default()
        };
        let engine = RuleEngine::new(rules, safety);

        assert_eq!(
            engine.classify_path(Path::new("/data/local/tmp/base.apk")),
            JunkType::TempApk
        );
        assert_eq!(
            engine.classify_path(Path::new("/data/local/tmp/split_config.arm64_v8a.apk")),
            JunkType::TempApk
        );

        assert_eq!(
            engine.classify_path(Path::new("/data/local/tmp/cleaner.sock")),
            JunkType::Ignored
        );
        assert_eq!(
            engine.classify_path(Path::new("/data/local/tmp/frida-server")),
            JunkType::Ignored
        );
        assert_eq!(
            engine.classify_path(Path::new("/data/local/tmp/script.sh")),
            JunkType::Ignored
        );
    }
}
