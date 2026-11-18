//! Command-line definition.
//!
//! Note: clap turns the `///` comments on the types and fields in this module
//! into `--help` text, so they are written for the examiner using the tool,
//! not for developers. Use `//` for developer notes here.

// Help text uses `<PATH>`-style placeholders, which rustdoc would otherwise
// flag as unclosed HTML tags.
#![allow(rustdoc::invalid_html_tags)]

use crate::hash::HashAlgorithm;
use crate::report_renderer::ReportFormat;
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

/// Logging utility for digital forensic investigation
#[derive(Parser)]
#[command(version)]
pub struct Cli {
    /// Run as if dflog was started in <PATH> instead of the current directory
    #[arg(short = 'C', global = true, value_name = "PATH")]
    pub chdir: Option<PathBuf>,

    // The selected subcommand.
    #[command(subcommand)]
    pub command: Command,
}

// One variant per subcommand. The `///` comment on each variant is the
// subcommand's description in `dflog --help`.
#[derive(Subcommand)]
pub enum Command {
    /// Export a report from the current journal
    Export(ExportArgs),
    /// Create a new dflog workspace
    Init(InitArgs),
    /// Print journal entries in a human-readable form
    Journal,
    /// Run a command and record it in the journal
    Run(RunArgs),
    /// Record external changes
    Record(RecordArgs),
    /// Verify the current state of the journal chain
    Verify,
}

// Arguments of `dflog init`.
#[derive(Args)]
pub struct InitArgs {
    /// Name of the examiner responsible for this case
    #[arg(long)]
    pub examiner: String,

    /// Case identifier
    #[arg(long)]
    pub case_id: String,

    /// Provide an initialization message
    #[arg(short, long)]
    pub message: Option<String>,

    /// Hash algorithm used for this case
    #[arg(long = "hash", value_enum, default_value_t)]
    pub hash: HashAlgorithm,

    /// Directory to create the workspace in
    #[arg(default_value = "./")]
    pub base_path: PathBuf,
}

// Arguments of `dflog run`
#[derive(Args)]
pub struct RunArgs {
    /// Reason for running this command
    #[arg(short, long)]
    pub message: String,

    /// File to hash and record even if not touched by the command
    #[arg(long = "artifact", value_name = "PATH")]
    pub artifacts: Vec<PathBuf>,

    /// The command to run, after '--'
    #[arg(last = true, required = true, value_name = "COMMAND")]
    pub command: Vec<String>,
}

#[derive(Args)]
pub struct RecordArgs {
    /// Reason for external changes
    #[arg(short, long)]
    pub message: String,
}

#[derive(Args)]
pub struct ExportArgs {
    #[arg(short, long)]
    pub format: Option<ReportFormat>,
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}
