#[cfg(test)]
mod tests {
    use cache_cleaner_daemon::config_pipeline::{EffectiveConfig, RawConfig, ValidatedConfig};
    use cache_cleaner_daemon::domain::candidate::Candidate;
    use cache_cleaner_daemon::domain::decision::{DecisionReason, PolicyDecision};
    use cache_cleaner_daemon::domain::target::{TargetClass, TargetDescriptor, TargetSafetyTier};
    use cache_cleaner_daemon::domain::types::{
        ByteCount, CandidateId, DeviceNumber, FileIdentity, GenerationId, InodeNumber,
        RelativePath, TargetId, UnixTimestamp,
    };
    use cache_cleaner_daemon::policy::PolicyEngine;
    use cache_cleaner_daemon::safety::SafetyGate;
    use std::path::PathBuf;

    fn create_test_context(pkg_name: Option<&str>, target_class: TargetClass) -> (TargetDescriptor, EffectiveConfig) {
        let descriptor = TargetDescriptor {
            target_id: TargetId::new("test:target"),
            target_class,
            base_path: PathBuf::from("/data/user/0/test/cache"),
            dev: DeviceNumber(1),
            ino: InodeNumber(100),
            owner_uid: 1000,
            owner_gid: 1000,
            package_name: pkg_name.map(|s| s.to_string()),
            safety_tier: TargetSafetyTier::StandardCache,
            catalog_generation: GenerationId::INITIAL,
        };

        let val_cfg = ValidatedConfig::from_raw(RawConfig {
            min_app_cache_age_days: Some(3),
            whitelist_packages: Some(vec!["com.google.android.gms".into(), "com.android.vending".into()]),
            ..Default::default()
        }).unwrap();

        let eff_cfg = EffectiveConfig::new(GenerationId::INITIAL, val_cfg);
        (descriptor, eff_cfg)
    }

    #[test]
    fn test_whitelist_safety() {
        let (descriptor, eff_cfg) = create_test_context(Some("com.google.android.gms"), TargetClass::AppCache);
        let candidate = Candidate {
            candidate_id: CandidateId(1),
            target_id: descriptor.target_id.clone(),
            rel_path: RelativePath::parse("temp_123.tmp").unwrap(),
            identity: FileIdentity::new(1, 101),
            size_bytes: ByteCount::new(1024),
            mtime: UnixTimestamp(100),
            atime: None,
            is_dir: false,
            is_symlink: false,
        };

        let safety = SafetyGate::new();
        let validated = safety.validate_candidate(candidate, &descriptor).unwrap();
        let policy = PolicyEngine::new();
        let decision = policy.evaluate_candidate(validated, &descriptor, &eff_cfg, UnixTimestamp(1_000_000));

        assert!(matches!(
            decision,
            PolicyDecision::Deny(deny) if deny.reason == DecisionReason::PackageWhitelisted
        ));
    }

    #[test]
    fn test_jit_art_bytecode_protection() {
        let (descriptor, eff_cfg) = create_test_context(Some("com.example.app"), TargetClass::AppCache);
        let candidate = Candidate {
            candidate_id: CandidateId(1),
            target_id: descriptor.target_id.clone(),
            rel_path: RelativePath::parse("compiled_view.dex").unwrap(),
            identity: FileIdentity::new(1, 101),
            size_bytes: ByteCount::new(1024),
            mtime: UnixTimestamp(100),
            atime: None,
            is_dir: false,
            is_symlink: false,
        };

        let safety = SafetyGate::new();
        let validated = safety.validate_candidate(candidate, &descriptor).unwrap();
        let policy = PolicyEngine::new();
        let decision = policy.evaluate_candidate(validated, &descriptor, &eff_cfg, UnixTimestamp(1_000_000));

        assert!(matches!(
            decision,
            PolicyDecision::Deny(deny) if deny.reason == DecisionReason::ProtectedBytecode
        ));
    }

    #[test]
    fn test_age_retention_policy_evaluation() {
        let (descriptor, eff_cfg) = create_test_context(Some("com.example.app"), TargetClass::AppCache);
        let now = UnixTimestamp::now();

        // 1. Fresh file (under 3 days) -> Denied by retention grace period
        let fresh_candidate = Candidate {
            candidate_id: CandidateId(1),
            target_id: descriptor.target_id.clone(),
            rel_path: RelativePath::parse("fresh.tmp").unwrap(),
            identity: FileIdentity::new(1, 101),
            size_bytes: ByteCount::new(1024),
            mtime: now, // 0 seconds old
            atime: None,
            is_dir: false,
            is_symlink: false,
        };

        let safety = SafetyGate::new();
        let validated_fresh = safety.validate_candidate(fresh_candidate, &descriptor).unwrap();
        let policy = PolicyEngine::new();
        let decision_fresh = policy.evaluate_candidate(validated_fresh, &descriptor, &eff_cfg, now);
        assert!(matches!(
            decision_fresh,
            PolicyDecision::Deny(deny) if deny.reason == DecisionReason::WithinRetentionGracePeriod
        ));

        // 2. Old file (over 3 days = 259,200s old) -> Allowed
        let old_mtime = UnixTimestamp(now.as_secs().saturating_sub(400_000));
        let old_candidate = Candidate {
            candidate_id: CandidateId(2),
            target_id: descriptor.target_id.clone(),
            rel_path: RelativePath::parse("old.tmp").unwrap(),
            identity: FileIdentity::new(1, 102),
            size_bytes: ByteCount::new(2048),
            mtime: old_mtime,
            atime: None,
            is_dir: false,
            is_symlink: false,
        };

        let validated_old = safety.validate_candidate(old_candidate, &descriptor).unwrap();
        let decision_old = policy.evaluate_candidate(validated_old, &descriptor, &eff_cfg, now);
        assert!(matches!(
            decision_old,
            PolicyDecision::Allow(permit) if permit.reason == DecisionReason::ExceedsRetentionAge
        ));
    }

    #[test]
    fn test_rustix_raw_dir_compilation() {
        use rustix::fs::{openat, FileType, Mode, OFlags, RawDir, CWD};
        use std::mem::MaybeUninit;

        let res = openat(
            CWD,
            ".",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        );
        if let Ok(dir_fd) = res {
            let mut buf = [MaybeUninit::uninit(); 4096];
            let mut raw_dir = RawDir::new(&dir_fd, &mut buf);
            let mut count = 0;
            while let Some(entry_res) = raw_dir.next() {
                if let Ok(entry) = entry_res {
                    let _name = entry.file_name();
                    let ft = entry.file_type();
                    assert!(matches!(
                        ft,
                        FileType::Directory
                            | FileType::Symlink
                            | FileType::RegularFile
                            | FileType::Unknown
                            | FileType::CharacterDevice
                            | FileType::BlockDevice
                            | FileType::Fifo
                            | FileType::Socket
                    ));
                    count += 1;
                }
            }
            assert!(count > 0);
        }
    }
}
