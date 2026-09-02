use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, RwLock};

use crate::domain::target::{TargetClass, TargetDescriptor, TargetSafetyTier};
use crate::domain::types::{GenerationId, TargetId};
use crate::error::Result;
use crate::fs::SafeDirHandle;

pub const KNOWN_CACHE_SUBDIRS: &[&str] = &[
    "cache",
    ".cache",
    "app_webview/Default/Cache",
    "app_webview/Default/Code Cache",
    "app_webview/Default/GPUCache",
    "app_textures",
    "splash_cache",
    "image_cache",
    "fresco_cache",
    "glide_cache",
    "coil_cache",
    "http-engine-cache",
    "picasso-cache",
    "volley",
    "disk_cache",
    "network_cache",
];

/// Known Android system junk locations
pub const SYSTEM_JUNK_LOCATIONS: &[(&str, TargetClass, TargetSafetyTier)] = &[
    ("/data/anr", TargetClass::Tombstones, TargetSafetyTier::StandardCache),
    ("/data/tombstones", TargetClass::Tombstones, TargetSafetyTier::StandardCache),
    ("/data/system/dropbox", TargetClass::LogArchive, TargetSafetyTier::StandardCache),
    ("/data/local/tmp", TargetClass::TempDir, TargetSafetyTier::StandardCache),
];

/// Immutable snapshot of the Target Catalog at a specific generation.
#[derive(Debug, Clone)]
pub struct CatalogSnapshot {
    pub generation: GenerationId,
    pub targets: HashMap<TargetId, TargetDescriptor>,
}

impl CatalogSnapshot {
    pub fn get(&self, target_id: &TargetId) -> Option<&TargetDescriptor> {
        self.targets.get(target_id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &TargetDescriptor> {
        self.targets.values()
    }

    pub fn len(&self) -> usize {
        self.targets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }
}

/// Dynamic Target Catalog managing platform discovery and authoritative target registration.
#[derive(Debug)]
pub struct TargetCatalog {
    current_generation: RwLock<GenerationId>,
    targets: RwLock<HashMap<TargetId, TargetDescriptor>>,
}

impl Default for TargetCatalog {
    fn default() -> Self {
        Self::new()
    }
}

impl TargetCatalog {
    pub fn new() -> Self {
        Self {
            current_generation: RwLock::new(GenerationId::INITIAL),
            targets: RwLock::new(HashMap::new()),
        }
    }

    /// Register a discovered target into the catalog.
    pub fn register_target(&self, descriptor: TargetDescriptor) {
        let mut targets = self.targets.write().unwrap();
        targets.insert(descriptor.target_id.clone(), descriptor);
    }

    /// Discovers standard Android cache directories under a user root path (e.g. `/data/user/0`).
    pub fn discover_android_user_targets(&self, user_base: &Path) -> Result<usize> {
        let mut count = 0;
        let read_dir = match fs::read_dir(user_base) {
            Ok(rd) => rd,
            Err(_) => return Ok(0),
        };

        let current_gen = *self.current_generation.read().unwrap();

        for entry in read_dir.flatten() {
            let pkg_path = entry.path();
            if !pkg_path.is_dir() {
                continue;
            }
            let pkg_name = match entry.file_name().into_string() {
                Ok(name) => name,
                Err(_) => continue,
            };

            for sub in KNOWN_CACHE_SUBDIRS {
                let cache_dir = pkg_path.join(sub);
                if cache_dir.exists() && cache_dir.is_dir() {
                    if let Ok(safe_handle) = SafeDirHandle::open_root(&cache_dir) {
                        let target_id_str = if *sub == "cache" {
                            format!("android:{}:cache", pkg_name)
                        } else {
                            format!("android:{}:{}:cache", pkg_name, sub.replace('/', "_"))
                        };
                        let descriptor = TargetDescriptor {
                            target_id: TargetId::new(target_id_str),
                            target_class: TargetClass::AppCache,
                            base_path: cache_dir,
                            dev: safe_handle.device(),
                            ino: safe_handle.inode(),
                            owner_uid: 0,
                            owner_gid: 0,
                            package_name: Some(pkg_name.clone()),
                            safety_tier: TargetSafetyTier::StandardCache,
                            catalog_generation: current_gen,
                        };
                        self.register_target(descriptor);
                        count += 1;
                    }
                }
            }
        }
        Ok(count)
    }

    /// Discovers system logs, crash dumps, and temp directory targets.
    pub fn discover_system_targets(&self) -> Result<usize> {
        let mut count = 0;
        let current_gen = *self.current_generation.read().unwrap();

        for (path_str, class, tier) in SYSTEM_JUNK_LOCATIONS {
            let path = Path::new(path_str);
            if path.exists() && path.is_dir() {
                if let Ok(safe_handle) = SafeDirHandle::open_root(path) {
                    let target_id_str = format!("system:{}", path_str.replace('/', "_"));
                    let descriptor = TargetDescriptor {
                        target_id: TargetId::new(target_id_str),
                        target_class: *class,
                        base_path: path.to_path_buf(),
                        dev: safe_handle.device(),
                        ino: safe_handle.inode(),
                        owner_uid: 0,
                        owner_gid: 0,
                        package_name: None,
                        safety_tier: *tier,
                        catalog_generation: current_gen,
                    };
                    self.register_target(descriptor);
                    count += 1;
                }
            }
        }
        Ok(count)
    }

    pub fn discover_all_targets(&self) {
        self.refresh_generation();
        let _ = self.discover_android_user_targets(Path::new("/data/user/0"));
        let _ = self.discover_android_user_targets(Path::new("/data/user_de/0"));
        let _ = self.discover_android_user_targets(Path::new("/data/data"));
        let _ = self.discover_system_targets();
    }

    pub fn register_target_simple(
        &self,
        target_id: &str,
        base_path: std::path::PathBuf,
        safety_tier: TargetSafetyTier,
        target_class: TargetClass,
        package_name: &str,
    ) {
        let current_gen = *self.current_generation.read().unwrap();
        let (dev, ino) = SafeDirHandle::open_root(&base_path)
            .map(|h| (h.device(), h.inode()))
            .unwrap_or_else(|_| (crate::domain::types::DeviceNumber(1), crate::domain::types::InodeNumber(1)));

        let descriptor = TargetDescriptor {
            target_id: TargetId::new(target_id),
            target_class,
            base_path,
            dev,
            ino,
            owner_uid: 0,
            owner_gid: 0,
            package_name: if package_name.is_empty() { None } else { Some(package_name.to_string()) },
            safety_tier,
            catalog_generation: current_gen,
        };
        self.register_target(descriptor);
    }

    /// Produces an immutable, generation-bound snapshot of all registered targets.
    pub fn take_snapshot(&self) -> Arc<CatalogSnapshot> {
        let targets = self.targets.read().unwrap().clone();
        let generation = *self.current_generation.read().unwrap();
        Arc::new(CatalogSnapshot {
            generation,
            targets,
        })
    }

    /// Increments catalog generation when discovery refreshes.
    pub fn refresh_generation(&self) -> GenerationId {
        let mut gen = self.current_generation.write().unwrap();
        *gen = gen.next();
        *gen
    }
}
