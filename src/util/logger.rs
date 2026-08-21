use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::LOG_PATH;

pub fn init_logger() {
    let mut builder = env_logger::Builder::from_default_env();

    builder.format(|buf, record| {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let log_line = format!(
            "[{ts}][{}][{}] {}",
            record.level(),
            record.target(),
            record.args()
        );

        // Append to /data/adb/cleaner/run/cleaner.log if run dir is present or accessible
        append_to_file(&log_line);

        writeln!(buf, "{log_line}")
    });

    if std::env::var("RUST_LOG").is_err() {
        builder.filter_level(log::LevelFilter::Info);
    }

    let _ = builder.try_init();
}

static FILE_LOCK: Mutex<()> = Mutex::new(());

fn append_to_file(line: &str) {
    let path = Path::new(LOG_PATH);
    if let Some(parent) = path.parent() {
        if parent.exists() {
            let _guard = FILE_LOCK.lock().unwrap_or_else(|p| p.into_inner());

            // Rotate log file if it exceeds 2MB
            if let Ok(meta) = fs::metadata(path) {
                if meta.len() > 2 * 1024 * 1024 {
                    let old_path = parent.join("cleaner.log.old");
                    let _ = fs::rename(path, old_path);
                }
            }

            if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
                let _ = writeln!(file, "{line}");
            }
        }
    }
}
