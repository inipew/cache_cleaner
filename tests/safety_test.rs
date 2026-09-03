#[cfg(test)]
mod tests {
    use cache_cleaner_daemon::domain::candidate::Candidate;
    use cache_cleaner_daemon::domain::target::{TargetClass, TargetDescriptor, TargetSafetyTier};
    use cache_cleaner_daemon::domain::types::{
        ByteCount, CandidateId, DeviceNumber, FileIdentity, CatalogGeneration, ConfigGeneration, InodeNumber,
        RelativePath, TargetId, UnixTimestamp,
    };
    use cache_cleaner_daemon::ipc::protocol::{CleanParams, Command};
    #[cfg(unix)]
    use cache_cleaner_daemon::ipc::server::is_command_authorized;
    use cache_cleaner_daemon::platform::{check_encryption_state, EncryptionState, StorageState};
    use cache_cleaner_daemon::safety::SafetyGate;
    use std::path::{Path, PathBuf};

    #[test]
    fn test_relative_path_safety_bounds() {
        assert!(RelativePath::parse("/etc/passwd").is_none());
        assert!(RelativePath::parse("../../../etc/shadow").is_none());
        assert!(RelativePath::parse("foo/../bar").is_none());
        assert!(RelativePath::parse("./foo").is_none());
        assert!(RelativePath::parse("").is_some());
        assert!(RelativePath::parse("cache/data.tmp").is_some());
    }

    #[test]
    fn test_safety_gate_target_safety_tiers() {
        let safety = SafetyGate::new();

        // 1. Target with StandardCache tier -> Allowed for validation
        let allowed_desc = TargetDescriptor {
            target_id: TargetId::new("test:allowed"),
            target_class: TargetClass::AppCache,
            base_path: PathBuf::from("/data/user/0/com.example.app/cache"),
            dev: DeviceNumber(1),
            ino: InodeNumber(100),
            owner_uid: 1000,
            owner_gid: 1000,
            package_name: Some("com.example.app".into()),
            safety_tier: TargetSafetyTier::StandardCache,
            catalog_generation: CatalogGeneration::INITIAL,
        };

        let cand = Candidate {
            candidate_id: CandidateId(1),
            target_id: allowed_desc.target_id.clone(),
            rel_path: RelativePath::parse("sub/file.tmp").unwrap(),
            identity: FileIdentity::new(1, 101),
            size_bytes: ByteCount::new(1024),
            mtime: UnixTimestamp::now(),
            atime: None,
            is_dir: false,
            is_symlink: false,
        };

        assert!(safety.validate_candidate(cand.clone(), &allowed_desc).is_ok());

        // 2. Target with ProtectedSystem or ReadOnlyInspection tier -> Absolute Deny by Safety Gate
        let protected_desc = TargetDescriptor {
            target_id: TargetId::new("test:protected"),
            target_class: TargetClass::AppCache,
            base_path: PathBuf::from("/system/app"),
            dev: DeviceNumber(1),
            ino: InodeNumber(200),
            owner_uid: 0,
            owner_gid: 0,
            package_name: None,
            safety_tier: TargetSafetyTier::ProtectedSystem,
            catalog_generation: CatalogGeneration::INITIAL,
        };

        let protected_cand = Candidate {
            target_id: protected_desc.target_id.clone(),
            ..cand.clone()
        };

        assert!(safety.validate_candidate(protected_cand, &protected_desc).is_err());
    }

    #[test]
    fn test_safety_gate_mount_crossing_denial() {
        let safety = SafetyGate::new();

        let desc = TargetDescriptor {
            target_id: TargetId::new("test:target"),
            target_class: TargetClass::AppCache,
            base_path: PathBuf::from("/data/user/0/com.example.app/cache"),
            dev: DeviceNumber(10), // Device 10
            ino: InodeNumber(100),
            owner_uid: 1000,
            owner_gid: 1000,
            package_name: Some("com.example.app".into()),
            safety_tier: TargetSafetyTier::StandardCache,
            catalog_generation: CatalogGeneration::INITIAL,
        };

        // Candidate on a different device (e.g. dev 99) -> Mount boundary violation!
        let mount_crossed_cand = Candidate {
            candidate_id: CandidateId(1),
            target_id: desc.target_id.clone(),
            rel_path: RelativePath::parse("mounted_sub/file.tmp").unwrap(),
            identity: FileIdentity::new(99, 101), // Dev 99
            size_bytes: ByteCount::new(1024),
            mtime: UnixTimestamp::now(),
            atime: None,
            is_dir: false,
            is_symlink: false,
        };

        assert!(safety.validate_candidate(mount_crossed_cand, &desc).is_err());
    }

    #[test]
    fn test_fail_closed_encryption_state() {
        let storage_state = StorageState::for_user(99999);
        if !Path::new("/data").exists() {
            assert!(storage_state.ce_available);
        } else {
            let enc_state = check_encryption_state(99999);
            assert_eq!(enc_state, EncryptionState::Unknown);
            assert!(!storage_state.ce_available);
            assert!(!storage_state.user_unlocked);
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_ipc_command_authorization_matrix() {
        // Root (0) is authorized for all commands
        assert!(is_command_authorized(0, &Command::GetStatus));
        assert!(is_command_authorized(0, &Command::StopDaemon));
        assert!(is_command_authorized(0, &Command::ReloadConfig));
        assert!(is_command_authorized(
            0,
            &Command::TriggerClean(CleanParams {
                deep: true,
                trim: true,
                zram_compact: true,
                dry_run: false,
            })
        ));

        // System (1000) is authorized for status, ping, and normal clean
        assert!(is_command_authorized(1000, &Command::GetStatus));
        assert!(is_command_authorized(1000, &Command::Ping));
        assert!(!is_command_authorized(1000, &Command::Cancel));
        assert!(is_command_authorized(
            1000,
            &Command::TriggerClean(CleanParams {
                deep: false,
                trim: false,
                zram_compact: false,
                dry_run: false,
            })
        ));

        // System (1000) is NOT authorized for deep clean or daemon termination
        assert!(!is_command_authorized(
            1000,
            &Command::TriggerClean(CleanParams {
                deep: true,
                trim: false,
                zram_compact: false,
                dry_run: false,
            })
        ));
        assert!(!is_command_authorized(1000, &Command::StopDaemon));

        // Shell (2000) is ONLY authorized for read-only queries
        assert!(is_command_authorized(2000, &Command::GetStatus));
        assert!(is_command_authorized(2000, &Command::GetStats));
        assert!(is_command_authorized(2000, &Command::Ping));
        assert!(!is_command_authorized(2000, &Command::Cancel));
        assert!(!is_command_authorized(
            2000,
            &Command::TriggerClean(CleanParams {
                deep: false,
                trim: false,
                zram_compact: false,
                dry_run: false,
            })
        ));
        assert!(!is_command_authorized(2000, &Command::StopDaemon));
        assert!(!is_command_authorized(2000, &Command::ReloadConfig));

        // Untrusted app UID (10123) is denied for all commands
        assert!(!is_command_authorized(10123, &Command::GetStatus));
        assert!(!is_command_authorized(10123, &Command::StopDaemon));
    }

    #[test]
    fn test_large_ipc_frame_safety() {
        use cache_cleaner_daemon::ipc::protocol::{read_message, send_message, Response};
        use std::io::Cursor;

        // Generate a large response message (> 128 KiB)
        let large_string = "x".repeat(128 * 1024);
        let resp = Response::Error(large_string.clone());

        let mut buffer = Vec::new();
        assert!(send_message(&mut buffer, &resp).is_ok());

        // Ensure buffer exceeds 64 KiB
        assert!(buffer.len() > 64 * 1024);

        // Verify successful roundtrip decoding
        let mut cursor = Cursor::new(buffer);
        let decoded: Response = read_message(&mut cursor).expect("Should decode large frame successfully");
        match decoded {
            Response::Error(msg) => assert_eq!(msg, large_string),
            _ => panic!("Expected Response::Error with matching payload"),
        }
    }

    #[test]
    fn test_platform_trait_abstraction_and_mockability() {
        use cache_cleaner_daemon::platform::{
            AndroidPlatform, AndroidSystemInfo, AndroidUser, EncryptionState, Platform, SelinuxMode,
        };
        use std::path::PathBuf;

        // 1. Production platform adapter
        let platform = AndroidPlatform;
        let users = platform.enumerate_users();
        assert!(!users.is_empty(), "Should enumerate at least user 0");
        let enc = platform.check_encryption_state(0);
        assert!(matches!(
            enc,
            EncryptionState::Unencrypted | EncryptionState::FullyUnlocked | EncryptionState::DeviceEncryptedOnly | EncryptionState::Unknown
        ));

        // 2. Mock platform implementation (demonstrating 100% hermetic test injection)
        struct MockPlatform;
        impl Platform for MockPlatform {
            fn enumerate_users(&self) -> Vec<AndroidUser> {
                vec![
                    AndroidUser {
                        user_id: 0,
                        ce_path: PathBuf::from("/mock/ce/0"),
                        de_path: PathBuf::from("/mock/de/0"),
                        media_path: PathBuf::from("/mock/media/0"),
                    },
                    AndroidUser {
                        user_id: 10,
                        ce_path: PathBuf::from("/mock/ce/10"),
                        de_path: PathBuf::from("/mock/de/10"),
                        media_path: PathBuf::from("/mock/media/10"),
                    },
                ]
            }

            fn check_encryption_state(&self, user_id: u32) -> EncryptionState {
                if user_id == 10 {
                    EncryptionState::DeviceEncryptedOnly
                } else {
                    EncryptionState::FullyUnlocked
                }
            }

            fn get_system_info(&self) -> AndroidSystemInfo {
                AndroidSystemInfo {
                    api_level: 34,
                    release_version: "14".into(),
                    manufacturer: "google".into(),
                    brand: "google".into(),
                    model: "Pixel 8".into(),
                    is_encrypted: true,
                }
            }

            fn get_selinux_mode(&self) -> SelinuxMode {
                SelinuxMode::Enforcing
            }
        }

        let mock = MockPlatform;
        let mock_users = mock.enumerate_users();
        assert_eq!(mock_users.len(), 2);
        assert_eq!(mock.check_encryption_state(10), EncryptionState::DeviceEncryptedOnly);
        assert_eq!(mock.get_selinux_mode(), SelinuxMode::Enforcing);
    }
}
