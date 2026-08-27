# Android Native Cache Cleaner & System Optimizer Daemon (Rust)

A pure and idiomatic Rust background daemon and CLI utility engineered specifically for the **Android Native ROM Architecture**, supporting **Android 9 (API 28) up to Android 16 (API 36+)**.

---

## 🔒 Mandatory & Fixed Runtime Directory Layout

All runtime files, sockets, locks, and logs are strictly fixed to:
```
/data/adb/cleaner/
├── bin/
│   └── cleaner                   # Primary Native Rust Executable
├── run/
│   ├── daemon                    # UNIX Domain Socket for IPC
│   ├── cleaner.log               # Daemon runtime log file (auto-rotation >2MB)
│   ├── cleaner.pid               # Active Process ID file
│   └── cleaner_daemon.lock       # Kernel advisory flock exclusive mutex
└── config.toml                   # Primary Daemon Configuration
```

---

## 🌟 Key Architecture & Highlights

- **Event-Driven Idle Sleep**: Operates strictly event-driven via Linux `epoll`, `timerfd`, and kernel Netlink `uevent`. Remains sleeping without polling loops and maintains a minimal memory footprint (< 3 MB RSS).
- **Zero-TOCTOU File Traversal**: Directory traversal is performed strictly using fd-relative syscalls (`openat`, `fstatat(SYMLINK_NOFOLLOW)`, and `unlinkat`) with same-filesystem mount boundary enforcement (`st_dev` verification), preventing symlink directory escapes and race conditions.
- **Preemptive I/O & CancellationToken**: The moment the user turns the screen on or interacts with the phone, ongoing I/O disk traversals yield and abort, minimizing impact on foreground UI responsiveness.
- **Kernel Governor & CGroup Isolation**: Automatically sets CPU scheduling to `SCHED_IDLE` / nice +19, I/O scheduling to `IOPRIO_CLASS_IDLE` (priority 7), and migrates the process into background cgroups (`/dev/cpuset/background` or `/sys/fs/cgroup/background`).
- **F2FS Native Garbage Collection**: Integrates with the Linux kernel F2FS sysfs subsystem (`/sys/fs/f2fs/*/gc_urgent`) to optimize NAND flash storage, restoring previous GC states upon exit.
- **Inter-Process Communication (IPC)**: Powered by UNIX Domain Sockets (`/data/adb/cleaner/run/daemon` and `@cleaner_daemon`) with `SO_PEERCRED` caller UID authentication and granular per-command RBAC authorization (Root, System, Shell).
- **Kernel-Level Single Instance Mutex**: Uses non-blocking `flock(LOCK_EX | LOCK_NB)` on `/data/adb/cleaner/run/cleaner_daemon.lock` and signalfd for clean lifecycle management.
- **Multi-User & FBE Storage State Aware**: Resolves multi-user profiles dynamically using `BTreeSet` and respects File-Based Encryption (FBE: DE vs CE), ensuring locked CE storage is never touched.

---

## 📂 Project Structure

```
cache_cleaner/
├── Cargo.toml                   # Optimized release profile & Android dependencies
├── build_android.sh             # Automated ARM64 compilation & Magisk ZIP packager
├── config/
│   └── cleaner.toml             # Default configuration & safety rules
├── init/
│   ├── action.sh                # KernelSU / APatch / Magisk manual clean action script
│   ├── cleaner_daemon.rc        # Android init.rc service descriptor (oneshot)
│   ├── service.sh               # Magisk late_start boot launcher
│   └── module.prop              # Magisk module metadata
├── src/
│   ├── main.rs                  # CLI & Daemon dual-mode entrypoint
│   ├── config.rs                # Configuration loader & mandatory path constants
│   ├── error.rs                 # Custom error types
│   ├── engine/                  # Core cleaning, walker, memory, storage & framework
│   │   ├── cancellation.rs      # Atomic CancellationToken
│   │   ├── framework.rs         # pm trim-caches & idle-maintenance bridge
│   │   ├── memory.rs            # ZRAM compaction & compact_memory
│   │   ├── rules.rs             # Typed Decision & safety whitelist engine
│   │   ├── storage.rs           # ioctl(FITRIM) NAND wear-leveling
│   │   └── walker.rs            # Fast recursive directory scanner & unlinker (fd-relative)
│   ├── hardware/                # Hardware & Kernel sensing
│   │   ├── f2fs.rs              # F2FS gc_urgent controller with RAII state restore
│   │   ├── thermal.rs           # Thermal zones & battery temp reader
│   │   └── uevent.rs            # Kernel Netlink power & screen watcher
│   ├── ipc/                     # Inter-Process Communication
│   │   ├── client.rs            # CLI client connector
│   │   ├── protocol.rs          # Length-prefixed JSON protocol & 64KB bounded framing
│   │   └── server.rs            # Non-blocking UnixListener with peer auth & command RBAC
│   ├── platform/                # Android platform specifics
│   │   ├── android_prop.rs      # System properties reader (SDK 28-36+)
│   │   ├── encryption.rs        # FBE (DE vs CE) decryption checker & StorageState
│   │   ├── selinux.rs           # SELinux mode & root validator
│   │   └── users.rs             # Multi-user & Private Space enumerator
│   ├── system/                  # Linux system & lifecycle
│   │   ├── cgroup.rs            # Background CGroup placement
│   │   ├── governor.rs          # SCHED_IDLE & IOPRIO_CLASS_IDLE
│   │   ├── pidfile.rs           # Kernel flock lockfile & PID manager
│   │   ├── signals.rs           # SignalFD handler (SIGTERM, SIGINT, SIGHUP)
│   │   └── watcher.rs           # Epoll event loop & daemon lifecycle
│   └── util/
│       ├── io_fast.rs           # Fast format and I/O utilities
│       └── logger.rs            # Dual console & rotating cleaner.log file logger
├── tests/                       # Unit, lifecycle, safety, and adversarial test suites
└── README.md
```

---

## 🚀 Usage

### 1. Lifecycle Commands
```bash
# Start the daemon in the background
/data/adb/cleaner/bin/cleaner start

# Check daemon status (State, Uptime, Screen state, Temperature, Freed bytes)
/data/adb/cleaner/bin/cleaner status

# Inspect rule engine decision and safety checks for a path
/data/adb/cleaner/bin/cleaner explain /data/data/com.whatsapp/cache

# Reload configuration without restarting (via SIGHUP / IPC)
/data/adb/cleaner/bin/cleaner reload

# Graceful restart
/data/adb/cleaner/bin/cleaner restart

# Stop the daemon completely (3-tier stop: IPC -> SIGTERM -> SIGKILL)
/data/adb/cleaner/bin/cleaner stop
```

### 2. Manual Cleaning
```bash
# Standard clean (safe app cache with 24h min age)
/data/adb/cleaner/bin/cleaner clean

# Deep clean (includes FITRIM storage wear-leveling and ZRAM compaction)
/data/adb/cleaner/bin/cleaner clean --deep --trim --zram

# Dry run (scan junk space and report breakdown without deleting)
/data/adb/cleaner/bin/cleaner clean --dry-run
```

### 3. Magisk / KernelSU / APatch Action Button
Press the **Action** button in the KernelSU / APatch Manager UI or run:
```bash
sh /data/adb/modules/native_cache_cleaner/action.sh
```
