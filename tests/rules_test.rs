#[cfg(test)]
mod tests {
    use cache_cleaner_daemon::config::{CleaningRulesConfig, SafetyConfig};
    use cache_cleaner_daemon::engine::rules::{JunkType, RuleEngine};
    use std::path::Path;

    #[test]
    fn test_whitelist_safety() {
        let rules = CleaningRulesConfig::default();
        let safety = SafetyConfig::default();
        let engine = RuleEngine::new(rules, safety);

        // Databases, shared_prefs, .nomedia must ALWAYS be ignored
        assert_eq!(
            engine.classify_path(Path::new("/data/data/com.whatsapp/databases/msgstore.db")),
            JunkType::Ignored
        );
        assert_eq!(
            engine.classify_path(Path::new("/data/data/com.whatsapp/shared_prefs/prefs.xml")),
            JunkType::Ignored
        );
        assert_eq!(
            engine.classify_path(Path::new("/sdcard/DCIM/.nomedia")),
            JunkType::Ignored
        );
        assert_eq!(
            engine.classify_path(Path::new(
                "/data/data/com.google.android.gms/files/auth_key"
            )),
            JunkType::Ignored
        );

        // Package names with substrings overlapping with protected patterns (e.g. "lib", "files", "database")
        // must NOT have their cache directory ignored
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
    }

    #[test]
    fn test_package_name_containing_cache_string_safety() {
        let rules = CleaningRulesConfig::default();
        let safety = SafetyConfig::default();
        let engine = RuleEngine::new(rules, safety);

        // Package named com.geocache.navigator - non-cache files MUST NOT be treated as junk
        assert_eq!(
            engine.classify_path(Path::new(
                "/data/data/com.geocache.navigator/databases/points.db"
            )),
            JunkType::Ignored
        );
        assert_eq!(
            engine.classify_path(Path::new(
                "/data/user/0/com.cachet.wallet/shared_prefs/keys.xml"
            )),
            JunkType::Ignored
        );
        assert_eq!(
            engine.classify_path(Path::new(
                "/data/data/com.geocache.navigator/files/track.gpx"
            )),
            JunkType::Ignored
        );

        // But its actual /cache folder SHOULD be AppCache
        assert_eq!(
            engine.classify_path(Path::new(
                "/data/data/com.geocache.navigator/cache/tile.png"
            )),
            JunkType::AppCache
        );
    }

    #[test]
    fn test_jit_art_bytecode_protection() {
        let rules = CleaningRulesConfig {
            clean_app_cache: true,
            clean_code_cache: false,
            ..Default::default()
        };
        let safety = SafetyConfig::default();
        let engine = RuleEngine::new(rules, safety);

        assert_eq!(
            engine.classify_path(Path::new(
                "/data/data/com.android.chrome/code_cache/test.dex"
            )),
            JunkType::Ignored
        );
        assert_eq!(
            engine.classify_path(Path::new(
                "/data/misc/profiles/cur/0/com.whatsapp/primary.prof"
            )),
            JunkType::Ignored
        );
        assert_eq!(
            engine.classify_path(Path::new("/data/dalvik-cache/arm64/boot.art")),
            JunkType::Ignored
        );
    }

    #[test]
    fn test_junk_classification() {
        let rules = CleaningRulesConfig {
            clean_app_cache: true,
            clean_webview_cache: true,
            clean_image_caches: true,
            clean_thumbnails: true,
            clean_code_cache: false,
            clean_oem_logs: true,
            clean_crash_dumps: true,
            keep_recent_crash_files: 3,
            clean_temp_apks: true,
            min_file_age_hours: 0,
        };
        let safety = SafetyConfig::default();
        let engine = RuleEngine::new(rules, safety);

        // Standard app cache
        assert_eq!(
            engine.classify_path(Path::new(
                "/data/data/com.instagram.android/cache/temp_123.tmp"
            )),
            JunkType::AppCache
        );

        // WebView cache
        assert_eq!(
            engine.classify_path(Path::new(
                "/data/data/com.example.app/app_webview/Default/Cache/data_0"
            )),
            JunkType::WebViewCache
        );

        // Image cache (Glide / Fresco / Coil)
        assert_eq!(
            engine.classify_path(Path::new(
                "/data/data/com.example.app/cache/image_cache/abc"
            )),
            JunkType::ImageCache
        );
        assert_eq!(
            engine.classify_path(Path::new(
                "/data/data/com.example.app/cache/fresco_cache/v2/entry"
            )),
            JunkType::ImageCache
        );

        // Thumbnails
        assert_eq!(
            engine.classify_path(Path::new("/sdcard/DCIM/.thumbnails/thumb_001.jpg")),
            JunkType::Thumbnail
        );

        // Multi-OEM logs
        assert_eq!(
            engine.classify_path(Path::new("/data/miui/gallery/log.txt")),
            JunkType::OemLog
        );
        assert_eq!(
            engine.classify_path(Path::new("/data/mqsas/crash.log")),
            JunkType::OemLog
        );
        assert_eq!(
            engine.classify_path(Path::new("/data/sec_log/dump.log")),
            JunkType::OemLog
        );
        assert_eq!(
            engine.classify_path(Path::new("/data/oppo/log/sys.log")),
            JunkType::OemLog
        );
        assert_eq!(
            engine.classify_path(Path::new("/data/vendor/mtklog/mobilelog.txt")),
            JunkType::OemLog
        );
        assert_eq!(
            engine.classify_path(Path::new("/data/log/hilog/hilog.001")),
            JunkType::OemLog
        );

        // Tombstones / ANR / DropBox
        assert_eq!(
            engine.classify_path(Path::new("/data/tombstones/tombstone_01")),
            JunkType::CrashDump
        );
        assert_eq!(
            engine.classify_path(Path::new("/data/anr/traces.txt")),
            JunkType::CrashDump
        );
        assert_eq!(
            engine.classify_path(Path::new(
                "/data/system/dropbox/data_app_crash@12345.txt.gz"
            )),
            JunkType::CrashDump
        );

        // Temp and staged APKs
        assert_eq!(
            engine.classify_path(Path::new("/data/app-staging/session_123.apk")),
            JunkType::TempApk
        );
        assert_eq!(
            engine.classify_path(Path::new("/data/local/tmp/base.apk")),
            JunkType::TempApk
        );
        assert_eq!(
            engine.classify_path(Path::new("/data/local/tmp/split.dex")),
            JunkType::TempApk
        );
    }

    #[test]
    fn test_rustix_raw_dir_compilation() {
        use rustix::fs::{openat, FileType, Mode, OFlags, RawDir, CWD};
        use std::mem::MaybeUninit;

        let res = openat(
            CWD,
            ".",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        );
        if let Ok(dir_fd) = res {
            let mut buf = [MaybeUninit::uninit(); 4096];
            let mut raw_dir = RawDir::new(&dir_fd, &mut buf);
            let mut count = 0;
            while let Some(entry_res) = raw_dir.next() {
                if let Ok(entry) = entry_res {
                    let _name = entry.file_name();
                    let ft = entry.file_type();
                    assert!(matches!(
                        ft,
                        FileType::Directory
                            | FileType::Symlink
                            | FileType::RegularFile
                            | FileType::Unknown
                            | FileType::CharacterDevice
                            | FileType::BlockDevice
                            | FileType::Fifo
                            | FileType::Socket
                    ));
                    count += 1;
                }
            }
            assert!(count > 0);
        }
    }
}

