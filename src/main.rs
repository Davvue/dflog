mod cli;
mod commands;
mod config;
mod error;
mod workspace;
mod hash;

use crate::cli::{Cli, Command};
use crate::commands::init;
use clap::Parser;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args = Cli::parse();

    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{} error: {}", env!("CARGO_PKG_NAME"), error);
            ExitCode::FAILURE
        }
    }
}

fn run(args: Cli) -> error::Result<()> {
    if let Some(dir) = &args.chdir {
        std::env::set_current_dir(dir)?;
    }

    match args.command {
        Command::Init(args) => init::run(args),
    }
}