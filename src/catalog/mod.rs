use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, RwLock};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use crate::domain::target::{TargetClass, TargetDescriptor, TargetSafetyTier};
use crate::domain::types::{CatalogGeneration, TargetId};
use crate::error::Result;
use crate::fs::SafeDirHandle;
use crate::platform::encryption::{check_encryption_state, EncryptionState};
use crate::platform::users::enumerate_users;

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

pub const SYSTEM_JUNK_LOCATIONS: &[(&str, TargetClass, TargetSafetyTier)] = &[
    ("/data/anr", TargetClass::Tombstones, TargetSafetyTier::StandardCache),
    ("/data/tombstones", TargetClass::Tombstones, TargetSafetyTier::StandardCache),
    ("/data/system/dropbox", TargetClass::LogArchive, TargetSafetyTier::StandardCache),
    ("/data/local/tmp", TargetClass::TempDir, TargetSafetyTier::StandardCache),
];

#[derive(Debug, Clone)]
pub struct CatalogSnapshot {
    pub generation: CatalogGeneration,
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

/// Dynamic Target Catalog with atomic snapshot swap — retains last valid on failure.
#[derive(Debug)]
pub struct TargetCatalog {
    snapshot: RwLock<Arc<CatalogSnapshot>>,
}

impl Default for TargetCatalog {
    fn default() -> Self {
        Self::new()
    }
}

impl TargetCatalog {
    pub fn new() -> Self {
        Self {
            snapshot: RwLock::new(Arc::new(CatalogSnapshot {
                generation: CatalogGeneration::INITIAL,
                targets: HashMap::new(),
            })),
        }
    }

    fn current_generation(&self) -> CatalogGeneration {
        self.snapshot.read().unwrap().generation
    }

    pub fn register_target(&self, descriptor: TargetDescriptor) {
        // For incremental registration (tests), insert into current snapshot copy
        let mut snap = self.snapshot.write().unwrap();
        let mut new_targets = snap.targets.clone();
        new_targets.insert(descriptor.target_id.clone(), descriptor);
        let new_snap = Arc::new(CatalogSnapshot {
            generation: snap.generation,
            targets: new_targets,
        });
        *snap = new_snap;
    }

    fn collect_android_user_targets(
        &self,
        user_base: &Path,
        user_id: u32,
        generation: CatalogGeneration,
        out: &mut HashMap<TargetId, TargetDescriptor>,
    ) -> usize {
        let mut count = 0;
        let read_dir = match fs::read_dir(user_base) {
            Ok(rd) => rd,
            Err(_) => return 0,
        };
        for entry in read_dir.flatten() {
            let pkg_path = entry.path();
            if !pkg_path.is_dir() {
                continue;
            }
            let pkg_name = match entry.file_name().into_string() {
                Ok(name) => {
                    if name.is_empty() || name.len() > 255 || name.contains('\0') {
                        continue;
                    }
                    name
                }
                Err(_) => continue,
            };
            // Enforce bounded package count
            if out.len() > 5000 {
                log::warn!("catalog discovery bounded: too many packages");
                break;
            }
            for sub in KNOWN_CACHE_SUBDIRS {
                let cache_dir = pkg_path.join(sub);
                if let Ok(safe_handle) = SafeDirHandle::open_root(&cache_dir) {
                    let target_id_str = if user_id == 0 {
                        if *sub == "cache" {
                            format!("android:{}:cache", pkg_name)
                        } else {
                            format!("android:{}:{}:cache", pkg_name, sub.replace('/', "_"))
                        }
                    } else if *sub == "cache" {
                        format!("android:u{}:{}:cache", user_id, pkg_name)
                    } else {
                        format!("android:u{}:{}:{}:cache", user_id, pkg_name, sub.replace('/', "_"))
                    };
                    let (owner_uid, owner_gid) = {
                        #[cfg(unix)]
                        {
                            fs::metadata(&cache_dir).map(|m| (m.uid(), m.gid())).unwrap_or((0, 0))
                        }
                        #[cfg(not(unix))]
                        {
                            (0, 0)
                        }
                    };
                    let tid = match TargetId::try_new(target_id_str) {
                        Ok(id) => id,
                        Err(_) => continue,
                    };
                    // Deduplicate: keep most restrictive safety tier if conflict
                    if let Some(existing) = out.get(&tid) {
                        if existing.safety_tier as u8 <= TargetSafetyTier::StandardCache as u8 {
                            continue;
                        }
                    }
                    let descriptor = TargetDescriptor {
                        target_id: tid,
                        target_class: TargetClass::AppCache,
                        base_path: cache_dir,
                        dev: safe_handle.device(),
                        ino: safe_handle.inode(),
                        owner_uid,
                        owner_gid,
                        package_name: Some(pkg_name.clone()),
                        safety_tier: TargetSafetyTier::StandardCache,
                        catalog_generation: generation,
                    };
                    out.insert(descriptor.target_id.clone(), descriptor);
                    count += 1;
                }
            }
        }
        count
    }

    fn collect_android_external_targets(
        &self,
        media_base: &Path,
        user_id: u32,
        generation: CatalogGeneration,
        out: &mut HashMap<TargetId, TargetDescriptor>,
    ) -> usize {
        let mut count = 0;
        let data_dir = media_base.join("Android/data");
        let read_dir = match fs::read_dir(&data_dir) {
            Ok(rd) => rd,
            Err(_) => return 0,
        };
        for entry in read_dir.flatten() {
            let pkg_path = entry.path();
            if !pkg_path.is_dir() {
                continue;
            }
            let pkg_name = match entry.file_name().into_string() {
                Ok(name) => name,
                Err(_) => continue,
            };
            let cache_dir = pkg_path.join("cache");
            if let Ok(safe_handle) = SafeDirHandle::open_root(&cache_dir) {
                let target_id_str = format!("android:u{}:{}:ext_cache", user_id, pkg_name);
                let tid = match TargetId::try_new(target_id_str) {
                    Ok(id) => id,
                    Err(_) => continue,
                };
                let (owner_uid, owner_gid) = {
                    #[cfg(unix)]
                    {
                        fs::metadata(&cache_dir).map(|m| (m.uid(), m.gid())).unwrap_or((0, 0))
                    }
                    #[cfg(not(unix))]
                    {
                        (0, 0)
                    }
                };
                let descriptor = TargetDescriptor {
                    target_id: tid,
                    target_class: TargetClass::ExternalCache,
                    base_path: cache_dir,
                    dev: safe_handle.device(),
                    ino: safe_handle.inode(),
                    owner_uid,
                    owner_gid,
                    package_name: Some(pkg_name),
                    safety_tier: TargetSafetyTier::StandardCache,
                    catalog_generation: generation,
                };
                out.insert(descriptor.target_id.clone(), descriptor);
                count += 1;
            }
        }
        count
    }

    fn collect_system_targets(
        &self,
        generation: CatalogGeneration,
        out: &mut HashMap<TargetId, TargetDescriptor>,
    ) -> usize {
        let mut count = 0;
        for (path_str, class, tier) in SYSTEM_JUNK_LOCATIONS {
            let path = Path::new(path_str);
            if let Ok(safe_handle) = SafeDirHandle::open_root(path) {
                let target_id_str = format!("system:{}", path_str.replace('/', "_"));
                let tid = TargetId::new(target_id_str);
                let (owner_uid, owner_gid) = {
                    #[cfg(unix)]
                    {
                        fs::metadata(path).map(|m| (m.uid(), m.gid())).unwrap_or((0, 0))
                    }
                    #[cfg(not(unix))]
                    {
                        (0, 0)
                    }
                };
                let descriptor = TargetDescriptor {
                    target_id: tid,
                    target_class: *class,
                    base_path: path.to_path_buf(),
                    dev: safe_handle.device(),
                    ino: safe_handle.inode(),
                    owner_uid,
                    owner_gid,
                    package_name: None,
                    safety_tier: *tier,
                    catalog_generation: generation,
                };
                out.insert(descriptor.target_id.clone(), descriptor);
                count += 1;
            }
        }
        count
    }

    /// Atomic discovery: build staging map offline, then swap atomically. Retains last valid on empty failure.
    pub fn discover_all_targets(&self) {
        let next_gen = {
            let cur = self.current_generation();
            cur.next().unwrap_or_else(|_| {
                log::error!("CatalogGeneration overflow — retaining current");
                cur
            })
        };
        let mut staging: HashMap<TargetId, TargetDescriptor> = HashMap::new();
        let users = enumerate_users();
        let mut found_any_user = false;
        for user in &users {
            let enc_state = check_encryption_state(user.user_id);
            match enc_state {
                EncryptionState::FullyUnlocked | EncryptionState::Unencrypted => {
                    found_any_user = true;
                    let _ = self.collect_android_user_targets(&user.ce_path, user.user_id, next_gen, &mut staging);
                    let _ = self.collect_android_user_targets(&user.de_path, user.user_id, next_gen, &mut staging);
                    let _ = self.collect_android_external_targets(&user.media_path, user.user_id, next_gen, &mut staging);
                }
                EncryptionState::DeviceEncryptedOnly => {
                    found_any_user = true;
                    let _ = self.collect_android_user_targets(&user.de_path, user.user_id, next_gen, &mut staging);
                }
                EncryptionState::Unknown => {
                    log::warn!("User {} encryption state unknown; skipping for safety", user.user_id);
                }
            }
        }
        if !found_any_user {
            let _ = self.collect_android_user_targets(Path::new("/data/user/0"), 0, next_gen, &mut staging);
            let _ = self.collect_android_user_targets(Path::new("/data/user_de/0"), 0, next_gen, &mut staging);
            let _ = self.collect_android_user_targets(Path::new("/data/data"), 0, next_gen, &mut staging);
        }
        let _ = self.collect_system_targets(next_gen, &mut staging);
        // Atomic swap: if staging empty and previous was non-empty, retain previous (don't publish empty on transient failure)
        let should_publish = {
            let cur = self.snapshot.read().unwrap();
            !staging.is_empty() || cur.targets.is_empty()
        };
        if should_publish {
            let new_snap = Arc::new(CatalogSnapshot {
                generation: next_gen,
                targets: staging,
            });
            *self.snapshot.write().unwrap() = new_snap;
        } else {
            log::warn!("catalog discovery produced empty staging — retaining last valid snapshot");
        }
    }

    // Backwards compat wrappers
    pub fn discover_android_user_targets(&self, user_base: &Path) -> Result<usize> {
        let gen = self.current_generation();
        let mut staging = HashMap::new();
        let c = self.collect_android_user_targets(user_base, 0, gen, &mut staging);
        for (_, desc) in staging {
            self.register_target(desc);
        }
        Ok(c)
    }
    pub fn discover_android_user_targets_for_user(&self, user_base: &Path, user_id: u32) -> Result<usize> {
        let gen = self.current_generation();
        let mut staging = HashMap::new();
        let c = self.collect_android_user_targets(user_base, user_id, gen, &mut staging);
        for (_, desc) in staging {
            self.register_target(desc);
        }
        Ok(c)
    }
    pub fn discover_android_external_targets(&self, media_base: &Path, user_id: u32) -> Result<usize> {
        let gen = self.current_generation();
        let mut staging = HashMap::new();
        let c = self.collect_android_external_targets(media_base, user_id, gen, &mut staging);
        for (_, desc) in staging {
            self.register_target(desc);
        }
        Ok(c)
    }
    pub fn discover_system_targets(&self) -> Result<usize> {
        let gen = self.current_generation();
        let mut staging = HashMap::new();
        let c = self.collect_system_targets(gen, &mut staging);
        for (_, desc) in staging {
            self.register_target(desc);
        }
        Ok(c)
    }

    pub fn register_target_simple(
        &self,
        target_id: &str,
        base_path: std::path::PathBuf,
        safety_tier: TargetSafetyTier,
        target_class: TargetClass,
        package_name: &str,
    ) -> Result<()> {
        let current_gen = self.current_generation();
        let safe_handle = SafeDirHandle::open_root(&base_path)?;
        let (dev, ino) = (safe_handle.device(), safe_handle.inode());
        let (owner_uid, owner_gid) = {
            #[cfg(unix)]
            {
                fs::metadata(&base_path).map(|m| (m.uid(), m.gid())).unwrap_or((0, 0))
            }
            #[cfg(not(unix))]
            {
                (0, 0)
            }
        };
        let descriptor = TargetDescriptor {
            target_id: TargetId::new(target_id),
            target_class,
            base_path,
            dev,
            ino,
            owner_uid,
            owner_gid,
            package_name: if package_name.is_empty() { None } else { Some(package_name.to_string()) },
            safety_tier,
            catalog_generation: current_gen,
        };
        self.register_target(descriptor);
        Ok(())
    }

    pub fn take_snapshot(&self) -> Arc<CatalogSnapshot> {
        self.snapshot.read().unwrap().clone()
    }

    pub fn refresh_generation(&self) -> CatalogGeneration {
        let cur = self.current_generation();
        let next = cur.next().unwrap_or_else(|_| {
            log::error!("CatalogGeneration overflow — failing closed");
            cur
        });
        // Bump generation atomically via new empty snapshot with same targets but new gen
        {
            let mut snap = self.snapshot.write().unwrap();
            let new_snap = Arc::new(CatalogSnapshot {
                generation: next,
                targets: snap.targets.clone(),
            });
            *snap = new_snap;
        }
        next
    }
}
