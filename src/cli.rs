use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;
use crate::hash::HashAlgorithm;

#[derive(Parser)]
#[command(version, about)]
pub struct Cli {
    /// Run as if dflog was started in <PATH> instead of the current directory
    #[arg(short = 'C', global = true, value_name = "PATH")]
    pub chdir: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Create a new dflog workspace
    Init(InitArgs),
}

#[derive(Args)]
pub struct InitArgs {
    /// Name of the examiner responsible for this case
    #[arg(long)]
    pub examiner: String,

    /// Case identifier
    #[arg(long)]
    pub case_id: String,

    /// Hash algorithm used for this case
    #[arg(long="hash", value_enum, default_value_t)]
    pub hash: HashAlgorithm,

    #[arg(default_value = "./")]
    pub base_path: PathBuf,
}