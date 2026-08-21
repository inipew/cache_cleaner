use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum EncryptionState {
    Unencrypted,
    DeviceEncryptedOnly,  // DE is accessible, CE is locked
    FullyUnlocked,        // CE is accessible
}

pub fn check_encryption_state(user_id: u32) -> EncryptionState {
    let ce_user_dir = if user_id == 0 {
        "/data/data".to_string()
    } else {
        format!("/data/user/{}", user_id)
    };
    let de_user_dir = format!("/data/user_de/{}", user_id);

    let ce_path = Path::new(&ce_user_dir);
    let de_path = Path::new(&de_user_dir);


    // If neither exists, might be legacy Android or unencrypted root
    if !ce_path.exists() && !de_path.exists() {
        return EncryptionState::FullyUnlocked;
    }

    // Try reading CE directory and testing access to verify credential key is unlocked
    if ce_path.exists() {
        match std::fs::read_dir(ce_path) {
            Ok(mut entries) => {
                // If directory is empty, consider it accessible
                if let Some(Ok(entry)) = entries.next() {
                    // Try getting metadata to detect ENOKEY on encrypted files
                    if entry.metadata().is_ok() {
                        EncryptionState::FullyUnlocked
                    } else {
                        EncryptionState::DeviceEncryptedOnly
                    }
                } else {
                    EncryptionState::FullyUnlocked
                }
            }
            Err(_) => EncryptionState::DeviceEncryptedOnly,
        }
    } else {
        EncryptionState::FullyUnlocked
    }
}
