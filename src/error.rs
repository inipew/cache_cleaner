use std::fmt;
use std::io;

#[derive(Debug)]
#[allow(dead_code)]
pub enum CleanerError {
    Io(io::Error),
    Json(serde_json::Error),
    Toml(toml::de::Error),
    TomlSer(toml::ser::Error),
    Ipc(String),
    Config(String),
    DaemonAlreadyRunning(u32),
    DaemonNotRunning,
    Platform(String),
    Cancelled,
    Other(String),
}

impl fmt::Display for CleanerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CleanerError::Io(e) => write!(f, "I/O error: {}", e),
            CleanerError::Json(e) => write!(f, "JSON parse error: {}", e),
            CleanerError::Toml(e) => write!(f, "TOML decode error: {}", e),
            CleanerError::TomlSer(e) => write!(f, "TOML encode error: {}", e),
            CleanerError::Ipc(msg) => write!(f, "IPC error: {}", msg),
            CleanerError::Config(msg) => write!(f, "Configuration error: {}", msg),
            CleanerError::DaemonAlreadyRunning(pid) => {
                write!(f, "Daemon is already running with PID {}", pid)
            }
            CleanerError::DaemonNotRunning => write!(f, "Daemon is not running"),
            CleanerError::Platform(msg) => write!(f, "Platform error: {}", msg),
            CleanerError::Cancelled => write!(f, "Operation was cancelled / preempted"),
            CleanerError::Other(msg) => write!(f, "Error: {}", msg),
        }
    }
}

impl std::error::Error for CleanerError {}

impl From<io::Error> for CleanerError {
    fn from(err: io::Error) -> Self {
        CleanerError::Io(err)
    }
}

impl From<serde_json::Error> for CleanerError {
    fn from(err: serde_json::Error) -> Self {
        CleanerError::Json(err)
    }
}

impl From<toml::de::Error> for CleanerError {
    fn from(err: toml::de::Error) -> Self {
        CleanerError::Toml(err)
    }
}

impl From<toml::ser::Error> for CleanerError {
    fn from(err: toml::ser::Error) -> Self {
        CleanerError::TomlSer(err)
    }
}

pub type Result<T> = std::result::Result<T, CleanerError>;
