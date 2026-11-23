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
use clap::{Args, Parser, Subcommand, ValueHint};
use clap_complete::Shell;
use std::path::PathBuf;

/// Logging utility for digital forensic investigation
#[derive(Parser)]
#[command(version)]
pub struct Cli {
    /// Run as if dflog was started in <PATH> instead of the current directory
    #[arg(short = 'C', global = true, value_name = "PATH", value_hint = ValueHint::DirPath)]
    pub chdir: Option<PathBuf>,

    // The selected subcommand.
    #[command(subcommand)]
    pub command: Command,
}

// One variant per subcommand. The `///` comment on each variant is the
// subcommand's description in `dflog --help`.
#[derive(Subcommand)]
pub enum Command {
    /// Print a shell completion script to stdout
    Completions(CompletionsArgs),
    /// Export a report from the current journal
    Export(ExportArgs),
    /// Create a new dflog workspace
    Init(InitArgs),
    /// Print journal entries in a human-readable form
    Journal,
    /// Recreate the file index from the recorded journal history
    Reindex(ReindexArgs),
    /// Run a command and record it in the journal
    Run(RunArgs),
    /// Record changes made to files in working/ outside dflog
    Record(RecordArgs),
    /// Check the journal's integrity
    Verify,
}

// Arguments of `dflog init`.
#[derive(Args)]
pub struct InitArgs {
    /// Name of the examiner responsible for this case
    #[arg(long, value_hint = ValueHint::Other)]
    pub examiner: String,

    /// Case identifier
    #[arg(long, value_hint = ValueHint::Other)]
    pub case_id: String,

    /// Note to record with workspace creation
    #[arg(short, long, value_hint = ValueHint::Other)]
    pub message: Option<String>,

    /// Hash algorithm used for this case
    #[arg(long = "hash", value_enum, default_value_t)]
    pub hash: HashAlgorithm,

    /// Directory to create the workspace in
    #[arg(default_value = "./", value_hint = ValueHint::DirPath)]
    pub base_path: PathBuf,
}

// Arguments of `dflog run`
#[derive(Args)]
pub struct RunArgs {
    /// Reason for running this command
    #[arg(short, long, value_hint = ValueHint::Other)]
    pub message: String,

    /// The command to run, after '--'
    #[arg(last = true, required = true, value_name = "COMMAND", value_hint = ValueHint::CommandWithArguments)]
    pub command: Vec<String>,
}

#[derive(Args)]
pub struct RecordArgs {
    /// Describe the changes made and why
    #[arg(short, long, value_hint = ValueHint::Other)]
    pub message: String,
}

#[derive(Args)]
pub struct ExportArgs {
    /// Report format. Defaults to the extension of --output, or to text if no output is specified
    #[arg(short, long)]
    pub format: Option<ReportFormat>,

    /// File to write the report to. Defaults reports/ in the workspace, named after report.name_format in .dflog/config.toml
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

#[derive(Args)]
pub struct ReindexArgs {
    /// Reason for rebuilding the index
    #[arg(short, long, value_hint = ValueHint::Other)]
    pub message: String,
}

#[derive(Args)]
pub struct CompletionsArgs {
    /// Shell to generate completions for
    #[arg(value_enum)]
    pub shell: Shell,
}
