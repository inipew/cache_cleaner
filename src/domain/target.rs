use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

use crate::domain::types::{CatalogGeneration, DeviceNumber, InodeNumber, TargetId};

/// Semantic classification of filesystem targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TargetClass {
    /// Standard application cache directory (`cache/`)
    AppCache,
    /// Application code/bytecode cache (`code_cache/`)
    CodeCache,
    /// System crash dumps, tombstones, and ANR traces
    Tombstones,
    /// Media and gallery thumbnails cache
    MediaCache,
    /// GPU / Vulkan shader caches
    ShaderCache,
    /// Rotated log files and crash archives
    LogArchive,
    /// Ephemeral temporary files (`/data/local/tmp`, `/tmp`)
    TempDir,
    /// External storage app cache (`/Android/data/<pkg>/cache`)
    ExternalCache,
}

impl fmt::Display for TargetClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AppCache => write!(f, "AppCache"),
            Self::CodeCache => write!(f, "CodeCache"),
            Self::Tombstones => write!(f, "Tombstones"),
            Self::MediaCache => write!(f, "MediaCache"),
            Self::ShaderCache => write!(f, "ShaderCache"),
            Self::LogArchive => write!(f, "LogArchive"),
            Self::TempDir => write!(f, "TempDir"),
            Self::ExternalCache => write!(f, "ExternalCache"),
        }
    }
}

/// Safety level assigned to a target during catalog discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TargetSafetyTier {
    /// Safe to clean during normal idle cleaning.
    StandardCache,
    /// Only safe to clean when the owner application is frozen/stopped.
    RequiresColdApp,
    /// May only be inspected or reported, never mutated.
    ReadOnlyInspection,
    /// Core system path, strictly denied from any mutation.
    ProtectedSystem,
}

/// Authoritative descriptor for a registered filesystem target emitted by Platform discovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetDescriptor {
    pub target_id: TargetId,
    pub target_class: TargetClass,
    pub base_path: PathBuf,
    pub dev: DeviceNumber,
    pub ino: InodeNumber,
    pub owner_uid: u32,
    pub owner_gid: u32,
    pub package_name: Option<String>,
    pub safety_tier: TargetSafetyTier,
    pub catalog_generation: CatalogGeneration,
}

impl TargetDescriptor {
    pub fn is_mutation_allowed(&self) -> bool {
        match self.safety_tier {
            TargetSafetyTier::StandardCache | TargetSafetyTier::RequiresColdApp => true,
            TargetSafetyTier::ReadOnlyInspection | TargetSafetyTier::ProtectedSystem => false,
        }
    }
}
