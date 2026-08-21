use std::fs;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct AndroidSystemInfo {
    pub api_level: u32,
    pub release_version: String,
    pub manufacturer: String,
    pub brand: String,
    pub model: String,
    pub is_encrypted: bool,
}

impl AndroidSystemInfo {
    pub fn detect() -> Self {
        let api_level = get_property("ro.build.version.sdk")
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(28); // Fallback: Android 9 (API 28)

        let release_version = get_property("ro.build.version.release")
            .unwrap_or_else(|| "9.0".to_string());

        let manufacturer = get_property("ro.product.manufacturer")
            .unwrap_or_else(|| "unknown".to_string())
            .to_lowercase();

        let brand = get_property("ro.product.brand")
            .unwrap_or_else(|| "unknown".to_string())
            .to_lowercase();

        let model = get_property("ro.product.model")
            .unwrap_or_else(|| "Generic Android".to_string());

        let crypto_state = get_property("ro.crypto.state")
            .unwrap_or_else(|| "unencrypted".to_string());
        let is_encrypted = crypto_state == "encrypted";

        Self {
            api_level,
            release_version,
            manufacturer,
            brand,
            model,
            is_encrypted,
        }
    }
}

pub fn get_property(key: &str) -> Option<String> {
    // 1. Try libc __system_property_get if on Android
    #[cfg(target_os = "android")]
    {
        use std::ffi::{CStr, CString};
        use std::os::raw::c_char;

        extern "C" {
            fn __system_property_get(name: *const c_char, value: *mut c_char) -> i32;
        }

        if let Ok(c_key) = CString::new(key) {
            let mut val_buf = vec![0u8; 96]; // PROP_VALUE_MAX is 92 in bionic
            let len = unsafe {
                __system_property_get(
                    c_key.as_ptr(),
                    val_buf.as_mut_ptr() as *mut c_char,
                )
            };
            if len > 0 {
                if let Ok(c_str) = unsafe { CStr::from_ptr(val_buf.as_ptr() as *const c_char) }.to_str() {
                    return Some(c_str.trim().to_string());
                }
            }
        }
    }

    // 2. Fallback: Parse /system/build.prop or /default.prop
    let prop_files = [
        "/system/build.prop",
        "/vendor/build.prop",
        "/product/build.prop",
        "/system_ext/build.prop",
        "/default.prop",
    ];

    for file in &prop_files {
        if let Ok(content) = fs::read_to_string(file) {
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with('#') || trimmed.is_empty() {
                    continue;
                }
                if let Some((k, v)) = trimmed.split_once('=') {
                    if k.trim() == key {
                        return Some(v.trim().to_string());
                    }
                }
            }
        }
    }

    // 3. Fallback: getprop command execution
    if let Ok(output) = Command::new("getprop").arg(key).output() {
        if output.status.success() {
            let val = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !val.is_empty() {
                return Some(val);
            }
        }
    }

    None
}
