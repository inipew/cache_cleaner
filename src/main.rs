pub mod cli;
pub mod config;
pub mod engine;
pub mod error;
pub mod hardware;
pub mod ipc;
pub mod platform;
pub mod system;
pub mod util;

use clap::Parser;
use cli::{dispatch_command, Cli};
use util::init_logger;

fn main() {
    init_logger();
    let cli = Cli::parse();
    dispatch_command(cli);
}
