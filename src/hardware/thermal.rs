use crate::util::read_file_to_buf;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy)]
pub struct ThermalReport {
    pub max_soc_temp_c: f32,
    pub battery_temp_c: f32,
}

pub fn read_thermal() -> ThermalReport {
    let mut max_soc = 0.0f32;
    let mut battery_temp = 0.0f32;
    let mut buf = [0u8; 64];

    // 1. Read battery temperature from dynamic power_supply directory
    if let Ok(entries) = fs::read_dir("/sys/class/power_supply") {
        for entry in entries.flatten() {
            let path = entry.path();
            let temp_file = path.join("temp");
            if let Some(val_str) = read_file_to_buf(&temp_file, &mut buf) {
                if let Ok(raw_val) = val_str.trim().parse::<f32>() {
                    let temp_c = normalize_temp(raw_val);
                    if temp_c > 0.0 && temp_c < 100.0 {
                        battery_temp = temp_c;
                        break;
                    }
                }
            }
        }
    }

    // Fallback if dynamic read didn't find battery temp
    if battery_temp == 0.0 {
        let battery_temp_paths = [
            "/sys/class/power_supply/battery/temp",
            "/sys/class/power_supply/bms/temp",
            "/sys/class/power_supply/main/temp",
        ];

        for path in &battery_temp_paths {
            if let Some(val_str) = read_file_to_buf(Path::new(path), &mut buf) {
                if let Ok(raw_val) = val_str.trim().parse::<f32>() {
                    let temp_c = normalize_temp(raw_val);
                    if temp_c > 0.0 && temp_c < 100.0 {
                        battery_temp = temp_c;
                        break;
                    }
                }
            }
        }
    }

    // 2. Read thermal zones for SoC (CPU/GPU/SoC) across Qualcomm, MTK, Exynos, Tensor, Unisoc
    let mut fallback_max_temp = 0.0f32;

    if let Ok(entries) = fs::read_dir("/sys/class/thermal") {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with("thermal_zone") {
                    let temp_path = path.join("temp");
                    let type_path = path.join("type");

                    let mut type_buf = [0u8; 64];
                    let type_name = read_file_to_buf(&type_path, &mut type_buf)
                        .unwrap_or_default()
                        .to_lowercase();

                    let is_soc = type_name.contains("cpu")
                        || type_name.contains("soc")
                        || type_name.contains("tsens")
                        || type_name.contains("mtktscpu")
                        || type_name.contains("cluster")
                        || type_name.contains("ap-therm")
                        || type_name.contains("exynos")
                        || type_name.contains("tensor")
                        || type_name.contains("gpu");

                    if let Some(temp_str) = read_file_to_buf(&temp_path, &mut buf) {
                        if let Ok(raw_temp) = temp_str.trim().parse::<f32>() {
                            let temp_c = normalize_temp(raw_temp);

                            if temp_c > 0.0 && temp_c < 120.0 {
                                if is_soc {
                                    if temp_c > max_soc {
                                        max_soc = temp_c;
                                    }
                                } else if temp_c > fallback_max_temp {
                                    fallback_max_temp = temp_c;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if max_soc == 0.0 {
        max_soc = if fallback_max_temp > 0.0 {
            fallback_max_temp
        } else {
            35.0
        };
    }

    if battery_temp == 0.0 {
        battery_temp = 30.0;
    }

    ThermalReport {
        max_soc_temp_c: max_soc,
        battery_temp_c: battery_temp,
    }
}

/// Normalizes raw temperature integers (which may be millidegrees C e.g. 45000 or deci-degrees e.g. 450)
fn normalize_temp(raw: f32) -> f32 {
    if raw > 1000.0 {
        raw / 1000.0 // Millidegrees Celsius (e.g. 45200 -> 45.2 C)
    } else if raw > 150.0 {
        raw / 10.0 // Deci-degrees Celsius (e.g. 452 -> 45.2 C)
    } else {
        raw // Direct Celsius (e.g. 45.2 C)
    }
}
