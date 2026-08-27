use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "cache-cleaner",
    author = "Antigravity Developer",
    version = env!("CARGO_PKG_VERSION"),
    about = "Zero-Idle-Overhead Android Cache Cleaner & System Optimizer Daemon in Rust",
    long_about = "A high-performance, native ROM-friendly background cleaner daemon and CLI utility supporting Android 9 to 16."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Path to custom configuration file
    #[arg(short, long, global = true)]
    pub config: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Start the background cleaner daemon (if not already running)
    Start {
        /// Run in foreground instead of background detached process
        #[arg(short, long)]
        foreground: bool,
    },

    /// Run as background daemon directly (Used by Android init / supervisors)
    Daemon,

    /// Gracefully stop the running cleaner daemon (3-tier: IPC -> SIGTERM -> SIGKILL)
    Stop,

    /// Restart the cleaner daemon cleanly
    Restart {
        /// Run in foreground after restart
        #[arg(short, long)]
        foreground: bool,
    },

    /// Trigger a cache and junk cleaning pass (Uses IPC if daemon is active, else standalone)
    Clean {
        /// Perform deep cleaning (includes storage trimming and ZRAM compaction)
        #[arg(short, long)]
        deep: bool,

        /// Run FITRIM on mount points (/data, /cache)
        #[arg(short, long)]
        trim: bool,

        /// Compact ZRAM swap memory
        #[arg(short, long)]
        zram: bool,

        /// Dry-run mode: scan and calculate freed space without deleting files
        #[arg(short = 'n', long)]
        dry_run: bool,

        /// Output report in structured JSON format
        #[arg(long)]
        json: bool,
    },

    /// Query daemon status via IPC or PID lock
    Status,

    /// Query accumulated cleaning statistics via IPC
    Stats {
        /// Output in raw JSON format
        #[arg(long)]
        json: bool,
    },

    /// Cancel an ongoing cleaning operation in the running daemon
    Cancel,

    /// Reload daemon configuration without restarting (via IPC or SIGHUP)
    Reload,

    /// Explain rule engine classification and safety decision for a specific path
    Explain {
        /// Target path to inspect
        path: PathBuf,
    },

    /// Show detected Android platform, FBE encryption, users, and hardware info
    Info,

    /// Generate a default cleaner.toml configuration file
    ConfigInit {
        /// Destination path (defaults to ./cleaner.toml)
        #[arg(short, long, default_value = "cleaner.toml")]
        output: PathBuf,
    },
}
