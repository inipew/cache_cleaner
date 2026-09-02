use serde::{Deserialize, Serialize};

use crate::domain::types::{
    ByteCount, CandidateId, DeviceNumber, FileIdentity, InodeNumber, RelativePath, TargetId,
    UnixTimestamp,
};

/// Candidate item discovered by Scanner during filesystem traversal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub candidate_id: CandidateId,
    pub target_id: TargetId,
    pub rel_path: RelativePath,
    pub identity: FileIdentity,
    pub size_bytes: ByteCount,
    pub mtime: UnixTimestamp,
    pub atime: Option<UnixTimestamp>,
    pub is_dir: bool,
    pub is_symlink: bool,
}

impl Candidate {
    pub fn is_regular_file(&self) -> bool {
        !self.is_dir && !self.is_symlink
    }
}

/// Candidate that has passed the Safety Engine verification gate.
/// Confirms that the item resides strictly within the Target's TrustedRoot and does not cross mount points.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafetyValidatedCandidate {
    pub candidate: Candidate,
    pub trusted_root_dev: DeviceNumber,
    pub trusted_root_ino: InodeNumber,
    pub validated_at: UnixTimestamp,
}

impl SafetyValidatedCandidate {
    pub fn new(candidate: Candidate, root_dev: DeviceNumber, root_ino: InodeNumber) -> Self {
        Self {
            candidate,
            trusted_root_dev: root_dev,
            trusted_root_ino: root_ino,
            validated_at: UnixTimestamp::now(),
        }
    }
}
