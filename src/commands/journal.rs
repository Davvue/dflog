use crate::error::Result;
use crate::escape::esc;
use crate::journal::Journal;
use crate::report_renderer::text::TextRenderer;
use crate::workspace::Workspace;
use std::io::Write;
use std::process::ExitCode;

pub fn run() -> Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let ws = Workspace::discover(&cwd)?;
    let journal = Journal::for_workspace(&ws)?;

    let renderer = TextRenderer;
    let mut out = std::io::stdout().lock();

    for (i, entry) in journal.entries()?.into_iter().enumerate() {
        if i > 0 {
            writeln!(out)?;
        }
        match entry {
            Err(e) => {
                out.flush()?;
                // Parse errors can quote parts of the (untrusted) line.
                eprintln!("{}", esc(e));
            }
            Ok(entry) => renderer.render_entry(&entry, &mut out)?,
        }
    }

    Ok(ExitCode::SUCCESS)
}
