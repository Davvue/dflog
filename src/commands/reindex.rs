use std::process::ExitCode;
use crate::error::{Error, Result};
use crate::journal::{Journal, Verification};
use crate::workspace::Workspace;

pub fn run() -> Result<ExitCode> {
    let ws = Workspace::discover(&std::env::current_dir()?)?;
    let journal = Journal::for_workspace(&ws)?;
    let Verification::Valid { .. } = journal.verify()? else { return Err(Error::JournalCorrupt("".into())) };
    journal.check_appendable()?;


    Ok(ExitCode::SUCCESS)
}