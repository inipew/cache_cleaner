pub mod android_prop;
pub mod encryption;
pub mod privilege;
pub mod selinux;
pub mod users;

pub use android_prop::AndroidSystemInfo;
pub use encryption::{check_encryption_state, EncryptionState, StorageState};
pub use privilege::{
    APatchPlatform, CallerIdentity, KernelSUPlatform, MagiskPlatform, PlatformCapabilities,
    PrivilegePlatform,
};
pub use selinux::{get_selinux_mode, SelinuxMode};
pub use users::{enumerate_users, AndroidUser};

/// Unified platform abstraction trait for Android platform facilities.
/// Decouples OS syscalls from business logic and enables hermetic test mocking.
pub trait Platform: Send + Sync {
    fn enumerate_users(&self) -> Vec<AndroidUser>;
    fn check_encryption_state(&self, user_id: u32) -> EncryptionState;
    fn get_system_info(&self) -> AndroidSystemInfo;
    fn get_selinux_mode(&self) -> SelinuxMode;
}

/// Production Android platform adapter directly delegating to kernel / Android platform APIs.
#[derive(Debug, Default, Clone, Copy)]
pub struct AndroidPlatform;

impl Platform for AndroidPlatform {
    fn enumerate_users(&self) -> Vec<AndroidUser> {
        enumerate_users()
    }

    fn check_encryption_state(&self, user_id: u32) -> EncryptionState {
        check_encryption_state(user_id)
    }

    fn get_system_info(&self) -> AndroidSystemInfo {
        AndroidSystemInfo::detect()
    }

    fn get_selinux_mode(&self) -> SelinuxMode {
        get_selinux_mode()
    }
}
