use std::fmt;
use std::io;

#[derive(Debug)]
pub enum CleanerError {
    Io(io::Error),
    Json(serde_json::Error),
    Toml(toml::de::Error),
    TomlSer(toml::ser::Error),
    Ipc(String),
    Config(String),
    ConfigError(String),
    SafetyViolation(String),
    Storage(String),
    ResourceExhausted(String),
    PlanBudgetExceeded { count: usize, limit: usize },
    PlanValidationFailed(String),
    Internal(String),
    DaemonAlreadyRunning(u32),
    DaemonNotRunning,
    Platform(String),
    Cancelled,
    Other(String),
}

impl fmt::Display for CleanerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Json(e) => write!(f, "JSON parse error: {e}"),
            Self::Toml(e) => write!(f, "TOML decode error: {e}"),
            Self::TomlSer(e) => write!(f, "TOML encode error: {e}"),
            Self::Ipc(msg) => write!(f, "IPC error: {msg}"),
            Self::Config(msg) => write!(f, "Configuration error: {msg}"),
            Self::ConfigError(msg) => write!(f, "Configuration error: {msg}"),
            Self::SafetyViolation(msg) => write!(f, "Safety violation: {msg}"),
            Self::Storage(msg) => write!(f, "Storage error: {msg}"),
            Self::ResourceExhausted(msg) => write!(f, "Resource exhausted: {msg}"),
            Self::PlanBudgetExceeded { count, limit } => {
                write!(f, "Plan budget exceeded: candidate count {count} exceeds limit {limit}")
            }
            Self::PlanValidationFailed(msg) => write!(f, "Plan validation failed: {msg}"),
            Self::Internal(msg) => write!(f, "Internal error: {msg}"),
            Self::DaemonAlreadyRunning(pid) => {
                write!(f, "Daemon is already running with PID {pid}")
            }
            Self::DaemonNotRunning => write!(f, "Daemon is not running"),
            Self::Platform(msg) => write!(f, "Platform error: {msg}"),
            Self::Cancelled => write!(f, "Operation was cancelled / preempted"),
            Self::Other(msg) => write!(f, "Error: {msg}"),
        }
    }
}

impl std::error::Error for CleanerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Json(err) => Some(err),
            Self::Toml(err) => Some(err),
            Self::TomlSer(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for CleanerError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<serde_json::Error> for CleanerError {
    fn from(err: serde_json::Error) -> Self {
        Self::Json(err)
    }
}

impl From<toml::de::Error> for CleanerError {
    fn from(err: toml::de::Error) -> Self {
        Self::Toml(err)
    }
}

impl From<toml::ser::Error> for CleanerError {
    fn from(err: toml::ser::Error) -> Self {
        Self::TomlSer(err)
    }
}

pub type Result<T> = std::result::Result<T, CleanerError>;
