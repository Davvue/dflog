use crate::cli::ExportArgs;
use crate::error::Result;
use std::process::ExitCode;

pub fn run(args: ExportArgs) -> Result<ExitCode> {
    Ok(ExitCode::SUCCESS)
}
