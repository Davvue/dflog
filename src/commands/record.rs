use crate::cli::RecordArgs;
use crate::error::{Error, Result};
use crate::index::Index;
use crate::journal::{Entry, EntryKind, Journal};
use crate::workspace::Workspace;
use std::path::Path;
use std::process::ExitCode;

pub fn run(args: RecordArgs) -> Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let ws = Workspace::discover(&cwd)?;
    let journal = Journal::for_workspace(&ws)?;
    let alg = ws.config.hash.algorithm;
    let scope = Path::new(Workspace::working_scope());

    eprintln!("checking for file changes. this might take a while");
    let index_path = ws.index_path();
    let index =
        Index::load(&index_path)?.ok_or_else(|| Error::IndexNotFound(index_path.clone()))?;
    let current = index.snapshot(&ws.root, scope, alg)?;
    let changes = index.diff(&current);

    if changes.is_empty() {
        eprintln!("found no changes. not saving entry");
        return Ok(ExitCode::SUCCESS);
    }

    let mut entry = Entry::new(
        EntryKind::Record,
        &ws.config,
        args.message,
        jiff::Timestamp::now(),
    );
    entry.external_changes = changes;
    let entry = journal.append(entry)?;
    current.save(&index_path)?;
    eprintln!(
        "recorded as entry {} ({} file changes)",
        entry.seq,
        entry.external_changes.len()
    );

    Ok(ExitCode::SUCCESS)
}
