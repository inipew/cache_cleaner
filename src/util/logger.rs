use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::LOG_PATH;

const MAX_LOG_SIZE_BYTES: u64 = 1536 * 1024; // 1.5 MB per file (Max 3 files = ~4.5 MB total)

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

    let mut state_guard = FILE_STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);

    // Initialize or check file handle
    if state_guard.is_none() {
        let initial_size = fs::metadata(path).map_or(0, |m| m.len());
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
            // Multi-generation log rotation: .log.2 <- .log.1 <- .log
            // Swap the file handle in place while holding the mutex so other threads never
            // observe an inconsistent intermediate state (old handle + rotated-on-disk file).
            let log_1 = parent.join("cleaner.log.1");
            let log_2 = parent.join("cleaner.log.2");

            let _ = fs::remove_file(&log_2);
            let _ = fs::rename(&log_1, &log_2);
            let _ = fs::rename(path, &log_1);

            match OpenOptions::new().create(true).append(true).open(path) {
                Ok(file) => {
                    state.file = file;
                    state.current_size = 0;
                }
                Err(_) => {
                    // Keep the old handle; the current line is dropped but state stays consistent
                    return;
                }
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
