use crate::cli::ReindexArgs;
use crate::error::{Error, Result};
use crate::index::Index;
use crate::journal::{Entry, EntryKind, Journal, Verification};
use crate::workspace::Workspace;
use std::path::Path;
use std::process::ExitCode;

pub fn run(args: ReindexArgs) -> Result<ExitCode> {
    let ws = Workspace::discover(&std::env::current_dir()?)?;
    let journal = Journal::for_workspace(&ws)?;
    if let Verification::Broken { line, reason, .. } = journal.verify()? {
        return Err(Error::JournalCorrupt(format!("line {line}: {reason}")));
    };
    journal.check_appendable()?;

    let entries: Vec<Entry> = journal.entries()?.into_iter().collect::<Result<_>>()?;
    let expected = Index::replay(&entries);

    let started_at = jiff::Timestamp::now();
    let actual = Index::default().snapshot(
        &ws.root,
        Path::new(Workspace::working_scope()),
        ws.config.hash.algorithm,
    )?;

    let mut entry = Entry::new(EntryKind::Reindex, &ws.config, args.message, started_at);
    entry.external_changes = Index::diff_expected(&expected, &actual);
    let entry = journal.append(entry)?;

    actual.save(&ws.index_path())?;

    eprintln!(
        "rebuilt index ({} files), recorded as entry {} ({} changes not in the journal)",
        actual.entries.len(),
        entry.seq,
        entry.external_changes.len()
    );

    Ok(ExitCode::SUCCESS)
}
