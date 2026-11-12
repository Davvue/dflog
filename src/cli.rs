use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    Init(InitArgs)
}

#[derive(Args)]
pub struct InitArgs {
    #[arg(default_value = "./")]
    pub base_path: std::path::PathBuf,

    #[arg(short, long)]
    pub examiner_name: Option<String>,

    #[arg(short, long)]
    pub case_id: Option<String>,
}