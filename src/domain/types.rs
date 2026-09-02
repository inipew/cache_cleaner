use std::fmt;
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

/// Identifies a logical cleanup job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct JobId(pub u64);

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "job-{}", self.0)
    }
}

/// Identifies a single execution attempt of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AttemptId(pub u64);

impl fmt::Display for AttemptId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "attempt-{}", self.0)
    }
}

/// Identifies a specific planned/executed mutation or maintenance operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct OperationId(pub u64);

impl fmt::Display for OperationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "op-{}", self.0)
    }
}

/// Identifies a deterministic plan created by the planner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PlanId(pub u64);

impl fmt::Display for PlanId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "plan-{}", self.0)
    }
}

/// Identifies an execution worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WorkerId(pub u32);

impl fmt::Display for WorkerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "worker-{}", self.0)
    }
}

/// Identifies a registered target location/catalog entry.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TargetId(pub String);

impl TargetId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl fmt::Display for TargetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "target:{}", self.0)
    }
}

/// Identifies a discovered candidate item within a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CandidateId(pub u64);

impl fmt::Display for CandidateId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cand-{}", self.0)
    }
}

/// Generation counter for Target Catalog and Configuration snapshots.
/// Enforces generation binding so that stale plans cannot execute against newer snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GenerationId(pub u64);

impl GenerationId {
    pub const INITIAL: Self = Self(1);

    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

impl fmt::Display for GenerationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "gen-{}", self.0)
    }
}

/// Unique identifier for an authorization capability grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GrantId(pub u64);

impl fmt::Display for GrantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "grant-{}", self.0)
    }
}

/// Strongly-typed byte count representation preventing integer overflow and negative byte counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ByteCount(pub u64);

impl ByteCount {
    pub const ZERO: Self = Self(0);

    pub fn new(bytes: u64) -> Self {
        Self(bytes)
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }

    pub fn saturating_add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }

    pub fn saturating_sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }

    pub fn format_human(&self) -> String {
        const KB: u64 = 1024;
        const MB: u64 = KB * 1024;
        const GB: u64 = MB * 1024;
        let b = self.0;
        if b >= GB {
            format!("{:.2} GiB", b as f64 / GB as f64)
        } else if b >= MB {
            format!("{:.2} MiB", b as f64 / MB as f64)
        } else if b >= KB {
            format!("{:.2} KiB", b as f64 / KB as f64)
        } else {
            format!("{} B", b)
        }
    }
}

impl fmt::Display for ByteCount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.format_human())
    }
}

/// Strongly-typed Unix timestamp in seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct UnixTimestamp(pub u64);

impl UnixTimestamp {
    pub fn now() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self(secs)
    }

    pub fn as_secs(&self) -> u64 {
        self.0
    }

    pub fn from_secs(secs: u64) -> Self {
        Self(secs)
    }

    pub fn age_secs(&self, now: Self) -> u64 {
        now.0.saturating_sub(self.0)
    }
}

impl fmt::Display for UnixTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}s", self.0)
    }
}

/// Filesystem Inode Number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct InodeNumber(pub u64);

impl fmt::Display for InodeNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ino:{}", self.0)
    }
}

/// Filesystem Device Number (st_dev).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DeviceNumber(pub u64);

impl fmt::Display for DeviceNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "dev:{}", self.0)
    }
}

/// Inode and Device composite pair for exact filesystem identity verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileIdentity {
    pub dev: DeviceNumber,
    pub ino: InodeNumber,
}

impl FileIdentity {
    pub fn new(dev: u64, ino: u64) -> Self {
        Self {
            dev: DeviceNumber(dev),
            ino: InodeNumber(ino),
        }
    }
}

impl fmt::Display for FileIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}:{}]", self.dev, self.ino)
    }
}

/// Strongly-typed relative path guaranteed not to have leading slashes or parent traversals (`..`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RelativePath(PathBuf);

impl RelativePath {
    pub fn parse(path_str: &str) -> Option<Self> {
        if path_str.chars().any(|c| c.is_control() || c == '\0') {
            return None;
        }
        let path = Path::new(path_str);
        if path.is_absolute() {
            return None;
        }
        for component in path.components() {
            match component {
                std::path::Component::Normal(_) => {}
                _ => return None, // Reject '.', '..', Prefix, RootDir
            }
        }
        Some(Self(path.to_path_buf()))
    }

    pub fn empty() -> Self {
        Self(PathBuf::new())
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    pub fn as_str(&self) -> &str {
        self.0.to_str().unwrap_or("")
    }

    pub fn join(&self, sub: &str) -> Option<Self> {
        let sub_rel = Self::parse(sub)?;
        let mut new_path = self.0.clone();
        new_path.push(sub_rel.as_path());
        Some(Self(new_path))
    }
}

impl fmt::Display for RelativePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.display())
    }
}
