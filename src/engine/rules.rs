use std::path::Path;

use crate::config::{CleaningRulesConfig, SafetyConfig};

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

    /// Determines whether a given path is safe to delete and what kind of junk it is.
    pub fn classify_path(&self, path: &Path) -> JunkType {
        let path_str = path.to_string_lossy();

        // 1. Safety Whitelist Check: NEVER touch protected critical files/directories
        for protected in &self.safety.protected_substrings {
            // Check if any path component exactly matches the protected pattern or path contains it
            if path
                .components()
                .any(|c| c.as_os_str() == protected.as_str())
                || path_str.contains(protected)
            {
                return JunkType::Ignored;
            }
        }

        // Whitelist packages: Exact path segment match or prefix match
        let has_whitelisted_pkg = self.safety.whitelist_packages.iter().any(|pkg| {
            path.components().any(|c| {
                let s = c.as_os_str().to_string_lossy();
                s == pkg.as_str() || s.starts_with(&format!("{}.", pkg))
            })
        });

        // Whitelisted packages protect all files except standard /cache subfolders
        let is_in_cache_folder = path.components().any(|c| {
            let s = c.as_os_str().to_string_lossy();
            s == "cache" || s == "app_cache"
        });

        if has_whitelisted_pkg && !is_in_cache_folder {
            return JunkType::Ignored;
        }

        // 2. Strict JIT / ART Code Cache Protection
        // Ensure JIT bytecode (code_cache, oat, dalvik-cache, ART profiles) is NEVER cleaned unless explicitly enabled
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
            if self.rules.clean_code_cache {
                return JunkType::CodeCache;
            } else {
                return JunkType::Ignored;
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

        if is_webview_cache && self.rules.clean_webview_cache {
            return JunkType::WebViewCache;
        }

        // B. Common image & network cache libraries (Fresco, Glide, Coil, OkHttp, Picasso, Volley)
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

        if is_image_or_net_cache && self.rules.clean_image_caches {
            return JunkType::ImageCache;
        }

        // C. Thumbnail caches
        let is_thumbnail = path.components().any(|c| {
            let s = c.as_os_str().to_string_lossy();
            s == ".thumbnails"
                || s == ".thumb"
                || s == ".thumbcache"
                || s == ".video_thumbnails"
                || s == ".albumthumbs"
                || s == "micro_thumbnail"
        });

        if is_thumbnail && self.rules.clean_thumbnails {
            return JunkType::Thumbnail;
        }

        // D. App internal/external cache (component-aware: matches exact "cache" or "app_cache" directory)
        if is_in_cache_folder && self.rules.clean_app_cache {
            return JunkType::AppCache;
        }

        // E. OEM Vendor Logs (Anchored to system root directories)
        let is_oem_log_root = OEM_LOG_ROOTS.iter().any(|root| path.starts_with(root));
        if is_oem_log_root && self.rules.clean_oem_logs {
            return JunkType::OemLog;
        }

        // F. Crash Dumps, ANR, and DropBox (Anchored to system root directories)
        let is_crash_dump_root = CRASH_DUMP_ROOTS.iter().any(|root| path.starts_with(root));
        if is_crash_dump_root && self.rules.clean_crash_dumps {
            return JunkType::CrashDump;
        }

        // G. Temporary APKs & Staged APKs
        let is_staged_apk = STAGED_APK_ROOTS.iter().any(|root| path.starts_with(root));
        if is_staged_apk && self.rules.clean_temp_apks {
            return JunkType::TempApk;
        } else if path.starts_with("/data/local/tmp") && self.rules.clean_temp_apks {
            let is_temp_artifact = path_str.ends_with(".apk")
                || path_str.ends_with(".tmp")
                || path_str.ends_with(".apks")
                || path_str.ends_with(".xapk")
                || path_str.ends_with(".dex");
            if is_temp_artifact {
                return JunkType::TempApk;
            }
        }

        JunkType::Ignored
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
    fn test_jit_code_cache_is_strictly_protected_by_default() {
        let rules = CleaningRulesConfig {
            clean_app_cache: true,
            clean_code_cache: false, // Default: JIT is NEVER touched
            ..Default::default()
        };
        let safety = SafetyConfig::default();
        let engine = RuleEngine::new(rules, safety);

        // JIT code_cache paths must ALWAYS be Ignored
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

        // Package named "com.geocache.navigator" or "com.cachet.app" must NOT have its databases or files cleaned!
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

        // But its actual cache subfolder SHOULD be cleaned
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
        let safety = SafetyConfig::default();
        let engine = RuleEngine::new(rules, safety);

        // Files inside an app's files directory should NOT match OEM log rules even if naming overlaps
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

        // But actual system root OEM logs SHOULD match
        assert_eq!(
            engine.classify_path(Path::new("/data/miui/debug_log.txt")),
            JunkType::OemLog
        );
        assert_eq!(
            engine.classify_path(Path::new("/data/log/kernel_log.txt")),
            JunkType::OemLog
        );
    }

    #[test]
    fn test_protected_substrings_cannot_be_deleted() {
        let rules = CleaningRulesConfig::default();
        let safety = SafetyConfig::default();
        let engine = RuleEngine::new(rules, safety);

        // Protected files/folders must never be classified as junk
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
            engine.classify_path(Path::new("/data/media/0/Pictures/.nomedia")),
            JunkType::Ignored
        );
    }

    #[test]
    fn test_local_tmp_safety_rules() {
        let rules = CleaningRulesConfig {
            clean_temp_apks: true,
            ..Default::default()
        };
        let safety = SafetyConfig::default();
        let engine = RuleEngine::new(rules, safety);

        // Temp apk should be cleaned
        assert_eq!(
            engine.classify_path(Path::new("/data/local/tmp/base.apk")),
            JunkType::TempApk
        );
        assert_eq!(
            engine.classify_path(Path::new("/data/local/tmp/split_config.arm64_v8a.apk")),
            JunkType::TempApk
        );

        // Sockets, scripts, binaries must NOT be classified as TempApk
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
