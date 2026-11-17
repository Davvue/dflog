//! dflog: a logging utility for digital forensic investigation.
//!
//! dflog keeps a tamper-evident journal of every step an examiner takes in a
//! case workspace. Commands run through dflog are recorded with their exact
//! command line, timestamps, exit status, captured output and a required
//! reason, and the files they touch are hashed.
//!
//! Module overview:
//! - [`cli`]: command-line definition (clap)
//! - [`commands`]: one module per subcommand
//! - [`workspace`]: locating and creating the case workspace and its `.dflog` directory
//! - [`config`]: the per-case `config.toml`
//! - [`journal`]: the hash-chained journal of entries
//! - [`hash`]: hash algorithms and streaming file hashing
//! - [`error`]: the crate-wide error type

mod cli;
mod commands;
mod config;
mod error;
mod hash;
mod index;
mod journal;
mod report_renderer;
mod workspace;

use crate::cli::{Cli, Command};
use crate::commands::{export, init, record, run, verify};
use clap::Parser;
use std::process::ExitCode;

/// Parses the command line and runs the selected subcommand
fn main() -> ExitCode {
    let args = Cli::parse();

    run(args).unwrap_or_else(|error| {
        eprintln!("{} error: {}", env!("CARGO_PKG_NAME"), error);
        ExitCode::FAILURE
    })
}

/// Applies global options (`-C`) and dispatches to the subcommand.
fn run(args: Cli) -> error::Result<ExitCode> {
    if let Some(dir) = &args.chdir {
        std::env::set_current_dir(dir)?;
    }

    match args.command {
        Command::Export(args) => export::run(args),
        Command::Init(args) => init::run(args).map(|()| ExitCode::SUCCESS),
        Command::Journal => commands::journal::run(),
        Command::Record(args) => record::run(args),
        Command::Run(args) => run::run(args),
        Command::Verify => verify::run(),
    }
}
