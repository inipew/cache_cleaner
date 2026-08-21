use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::LOG_PATH;

const MAX_LOG_SIZE_BYTES: u64 = 2 * 1024 * 1024; // 2 MB

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

        // Efficiently append to /data/adb/cleaner/run/cleaner.log if run dir is present
        append_to_file(&log_line);

        writeln!(buf, "{log_line}")
    });

    if std::env::var("RUST_LOG").is_err() {
        builder.filter_level(log::LevelFilter::Info);
    }

    let _ = builder.try_init();
}

struct LogFileState {
    file: File,
    current_size: u64,
}

static FILE_STATE: Mutex<Option<LogFileState>> = Mutex::new(None);

fn append_to_file(line: &str) {
    let path = Path::new(LOG_PATH);
    let parent = match path.parent() {
        Some(p) if p.exists() => p,
        _ => return,
    };

    let mut state_guard = FILE_STATE.lock().unwrap_or_else(|p| p.into_inner());

    // Rotate or initialize file handle
    if state_guard.is_none() {
        let initial_size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if let Ok(file) = OpenOptions::new().create(true).append(true).open(path) {
            *state_guard = Some(LogFileState {
                file,
                current_size: initial_size,
            });
        }
    }

    if let Some(ref mut state) = *state_guard {
        let line_len = line.len() as u64 + 1;

        if state.current_size + line_len > MAX_LOG_SIZE_BYTES {
            // Rotate log file
            drop(state_guard.take());
            let old_path = parent.join("cleaner.log.old");
            let _ = fs::rename(path, old_path);

            if let Ok(file) = OpenOptions::new().create(true).append(true).open(path) {
                *state_guard = Some(LogFileState {
                    file,
                    current_size: 0,
                });
            }
        }
    }

    if let Some(ref mut state) = *state_guard {
        if writeln!(state.file, "{line}").is_ok() {
            state.current_size += line.len() as u64 + 1;
            let _ = state.file.flush();
        }
    }
}
