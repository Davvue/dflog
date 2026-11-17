use crate::error::Result;
use crate::journal::Journal;
use crate::workspace::Workspace;
use std::process::ExitCode;

pub fn run() -> Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let ws = Workspace::discover(&cwd)?;
    let journal = Journal::for_workspace(&ws)?;

    for entry in journal.entries()? {
        match entry {
            Err(e) => {
                eprintln!("{e}");
            }
            Ok(entry) => {
                println!("Journal entry {}", entry.seq);
                println!("\tMessage: {}", entry.message);
                println!("\tKind: {}", entry.kind.name());

                if !entry.changes.is_empty() {
                    println!("\tChanges:");

                    for change in entry.changes {
                        println!("\t\t{}: {}", change.path.display(), change.change);
                    }
                }
                println!();
            }
        }
    }

    Ok(ExitCode::SUCCESS)
}
