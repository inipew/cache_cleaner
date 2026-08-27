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

    // On Android, user directories must exist. If neither exists, fail-closed as Unknown.
    if !ce_path.exists() && !de_path.exists() {
        return EncryptionState::Unknown;
    }

    // Try reading CE directory and testing access to verify credential key is unlocked
    if ce_path.exists() {
        match std::fs::read_dir(ce_path) {
            Ok(mut entries) => {
                if let Some(Ok(entry)) = entries.next() {
                    // Try getting metadata to detect ENOKEY (key revoked / locked) on encrypted files
                    if entry.metadata().is_ok() {
                        EncryptionState::FullyUnlocked
                    } else {
                        EncryptionState::DeviceEncryptedOnly
                    }
                } else {
                    // Empty directory: check DE presence to confirm valid unlocked structure
                    if de_path.exists() {
                        EncryptionState::FullyUnlocked
                    } else {
                        EncryptionState::Unknown
                    }
                }
            }
            Err(_) => EncryptionState::DeviceEncryptedOnly,
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
