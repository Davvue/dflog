mod cli;
mod commands;

use clap::Parser;
use crate::cli::{Cli, Command};
use crate::commands::init;

fn main() {
    let args = Cli::parse();

    match args.command {
        Command::Init(args) => init::run(args)
    };
}
