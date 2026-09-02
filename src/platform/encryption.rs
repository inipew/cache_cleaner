use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncryptionState {
    Unencrypted,
    DeviceEncryptedOnly, // DE is accessible, CE is locked
    FullyUnlocked,       // CE is accessible
    Unknown,             // Unverified/Ambiguous -> fail-closed (NEVER clean)
}

#[must_use]
pub fn check_encryption_state(user_id: u32) -> EncryptionState {
    let ce_user_dir = if user_id == 0 {
        "/data/data".to_string()
    } else {
        format!("/data/user/{user_id}")
    };
    let de_user_dir = format!("/data/user_de/{user_id}");

    let ce_path = Path::new(&ce_user_dir);
    let de_path = Path::new(&de_user_dir);

    // If on a non-Android / test environment where /data doesn't exist
    if !Path::new("/data").exists() {
        return EncryptionState::Unencrypted;
    }

    // Determine whether the system reports a file-based-encrypted device. Cross-checking the
    // vold-reported crypto state prevents a permissive `statx`/`metadata()` result from a keyed
    // (but still "readable") CE path from being misreported as FullyUnlocked when the key is not
    // actually loaded.
    let crypto_state = crate::platform::android_prop::get_property("ro.crypto.state")
        .unwrap_or_else(|| "unencrypted".to_string());
    let crypto_type = crate::platform::android_prop::get_property("ro.crypto.type");
    let reports_fbe = crypto_state == "encrypted"
        && crypto_type
            .as_deref()
            .is_none_or(|t| t == "file" || t == "filesystem");

    // On Android, user directories must exist. If neither exists, fail-closed as Unknown.
    if !ce_path.exists() && !de_path.exists() {
        return EncryptionState::Unknown;
    }

    // Try reading CE directory and testing access to verify credential key is unlocked
    if ce_path.exists() {
        // Inspect the error kind of the first few entries. Only a definitive
        // ENOKEY-style lock (or access failure on a crypto block) means "locked".
        let read_entries = match std::fs::read_dir(ce_path) {
            Ok(e) => e,
            Err(_) => {
                return if de_path.exists() {
                    EncryptionState::DeviceEncryptedOnly
                } else {
                    EncryptionState::Unknown
                };
            }
        };

        let mut saw_ok = false;
        let mut saw_locked = false;
        for entry in read_entries.flatten().take(8) {
            match entry.metadata() {
                Ok(_) => saw_ok = true,
                Err(e) => {
                    let errno = e.raw_os_error();
                    // ENOKEY (126) is the canonical "credential key not loaded" errno from
                    // Linux fscrypt/ext4; also treat EACCES/EPERM from a locked key dir as locked
                    if errno == Some(126)
                        || errno == Some(libc::EACCES)
                        || errno == Some(libc::EPERM)
                        || errno == Some(libc::EIO)
                    {
                        saw_locked = true;
                    }
                }
            }
            if saw_locked {
                break;
            }
        }

        // If any entry definitively failed as locked, CE is not accessible.
        if saw_locked {
            return if de_path.exists() {
                EncryptionState::DeviceEncryptedOnly
            } else {
                EncryptionState::Unknown
            };
        }

        // If the device reports FBE but we saw no definitive lock AND no entry we could read,
        // be conservative: a truly unlocked CE dir has readable entries. Fail closed rather than
        // claiming FullyUnlocked without confirming read access.
        if saw_ok {
            EncryptionState::FullyUnlocked
        } else if reports_fbe && de_path.exists() {
            // Empty or unreadable CE dir on a reported-encrypted device -> cannot confirm unlock
            EncryptionState::DeviceEncryptedOnly
        } else if de_path.exists() {
            EncryptionState::DeviceEncryptedOnly
        } else {
            EncryptionState::Unknown
        }
    } else if de_path.exists() {
        EncryptionState::DeviceEncryptedOnly
    } else {
        EncryptionState::Unknown
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageState {
    pub ce_available: bool,
    pub de_available: bool,
    pub user_unlocked: bool,
}

impl StorageState {
    #[must_use]
    pub fn for_user(user_id: u32) -> Self {
        let enc_state = check_encryption_state(user_id);
        // Fail-closed: CE is ONLY accessible if explicitly FullyUnlocked or Unencrypted
        let ce_available = matches!(enc_state, EncryptionState::FullyUnlocked | EncryptionState::Unencrypted);
        let de_available = matches!(
            enc_state,
            EncryptionState::FullyUnlocked | EncryptionState::DeviceEncryptedOnly | EncryptionState::Unencrypted
        );
        let user_unlocked = ce_available;
        Self {
            ce_available,
            de_available,
            user_unlocked,
        }
    }
}
