mod cli;
mod commands;

use crate::cli::{Cli, Command};
use crate::commands::init;
use clap::Parser;

fn main() {
    let args = Cli::parse();

    let execution_result = match args.command {
        Command::Init(args) => init::run(args),
    };

    match execution_result {
        Ok(_) => (),
        Err(error) => eprintln!(
            "{} error: {}",
            option_env!("CARGO_PKG_NAME").unwrap_or("dflog"),
            error
        ),
    }
}
