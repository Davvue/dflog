use crate::error::Result;
use crate::escape::esc;
use crate::journal::{Journal, Verification};
use crate::workspace::Workspace;
use std::process::ExitCode;

pub fn run() -> Result<ExitCode> {
    let ws = Workspace::discover(&std::env::current_dir()?)?;
    let journal = Journal::for_workspace(&ws)?;

    match journal.verify()? {
        Verification::Valid { entries, head } => {
            println!("Journal valid.");
            println!("\tFound {} entries", entries);
            println!("\tCurrent head is {}", head);
            Ok(ExitCode::SUCCESS)
        }
        Verification::Broken { line, seq, reason } => {
            println!("Journal broken.");
            // Parse errors can quote parts of the (untrusted) line.
            let reason = esc(reason);

            if let Some(checked_seq) = seq {
                println!("Line {} (seq {}): {}", line, checked_seq, reason);
            } else {
                println!("Line {}: {}", line, reason);
            }

            Ok(ExitCode::FAILURE)
        }
    }
}
