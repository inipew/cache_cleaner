use cache_cleaner_daemon::cli::{dispatch_command, Cli};
use cache_cleaner_daemon::util::init_logger;
use clap::Parser;

fn main() {
    init_logger();
    let cli = Cli::parse();
    dispatch_command(cli);
}

