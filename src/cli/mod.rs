pub mod args;
pub mod clean;
pub mod daemon_ctrl;
pub mod diagnostics;
pub mod output;

use crate::config::DaemonConfig;
use crate::system::DaemonRunner;
pub use args::{Cli, Commands};

pub fn dispatch_command(cli: Cli) {
    let (config, active_config_path) = DaemonConfig::load_or_default_with_path(cli.config.as_ref());

    match cli.command.unwrap_or(Commands::Status) {
        Commands::Start { foreground } => {
            daemon_ctrl::start_daemon(active_config_path.as_ref(), foreground);
        }

        Commands::Daemon => {
            let mut runner = DaemonRunner::new(config, active_config_path);
            runner.run();
        }

        Commands::Stop => {
            daemon_ctrl::stop_daemon();
        }

        Commands::Restart { foreground } => {
            daemon_ctrl::restart_daemon(active_config_path.as_ref(), foreground);
        }

        Commands::Clean {
            deep,
            trim,
            zram,
            dry_run,
            json,
        } => {
            clean::execute_clean(config, deep, trim, zram, dry_run, json);
        }

        Commands::Status => {
            diagnostics::show_status();
        }

        Commands::Stats { json } => {
            diagnostics::show_stats(json);
        }

        Commands::Cancel => {
            daemon_ctrl::cancel_operation();
        }

        Commands::Reload => {
            daemon_ctrl::reload_config();
        }

        Commands::Explain { path } => {
            diagnostics::explain_path(&config, &path);
        }

        Commands::Idle { explain, json } => {
            diagnostics::show_idle_assessment(explain, json);
        }

        Commands::Info => {
            diagnostics::show_platform_info();
        }

        Commands::ConfigInit { output } => {
            diagnostics::init_config(&output);
        }
    }
}
