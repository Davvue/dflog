//! Plain-text report and the entry listing of `dflog journal`.
//!
//! Every value taken from the journal is untrusted and goes through
//! [`esc`], see [`crate::escape`].

use crate::error::Result;
use crate::escape::esc;
use crate::journal::{Change, CommandOutput, Entry, EntryKind, FileChange, FileStat, Verification};
use crate::report_renderer::format::{digest, local_time};
use crate::report_renderer::{Renderer, Report};
use std::io::Write;

/// Plain-text report, meant to be read by people and attached to case notes.
pub struct TextRenderer;

impl Renderer for TextRenderer {
    fn render(&self, report: &Report, out: &mut dyn Write) -> Result<()> {
        writeln!(
            out,
            "Report for case '{}' by examiner '{}'",
            esc(&report.case_id),
            esc(&report.examiner)
        )?;
        writeln!(
            out,
            "Generated at {} using dflog version {}",
            report.generated_at, report.dflog_version
        )?;
        writeln!(out, "Journal file hash: {}", report.journal_hash)?;
        match &report.verification {
            Verification::Valid { entries, head } => {
                writeln!(
                    out,
                    "Log verification passed: {} entries found, head hash is {}",
                    entries, head
                )?;
            }
            Verification::Broken { line, seq, reason } => {
                writeln!(
                    out,
                    "Log verification failed: '{}' at line {} (sequence {})",
                    esc(reason),
                    line,
                    seq.map_or("N/A".into(), |s| s.to_string())
                )?;
                writeln!(
                    out,
                    "The following report may not be valid. Proceed with caution"
                )?;
            }
        }
        writeln!(out)?;
        writeln!(out, "--- Journal entries ---")?;

        for entry in &report.entries {
            writeln!(out)?;
            match entry {
                Ok(entry) => self.render_entry(entry, out)?,
                Err(e) => writeln!(out, "invalid entry: {}", esc(e))?,
            }
        }

        Ok(())
    }
}

impl TextRenderer {
    /// Writes one journal entry, without a separating blank line. Used for
    /// each entry of a report and by `dflog journal`.
    pub fn render_entry(&self, entry: &Entry, out: &mut dyn Write) -> Result<()> {
        let offset = entry.host.utc_offset_seconds;

        writeln!(out, "Entry {}", entry.seq)?;
        writeln!(out, "\tMessage: {}", esc(&entry.message))?;
        writeln!(out, "\tExaminer: {}", esc(&entry.examiner))?;
        writeln!(out, "\tStarted: {}", local_time(entry.started_at, offset))?;
        writeln!(out, "\tFinished: {}", local_time(entry.finished_at, offset))?;
        writeln!(
            out,
            "\tHost: {}",
            esc(format_args!(
                "{} (user {}, {}/{}, dflog {})",
                entry.host.hostname,
                entry.host.user,
                entry.host.os,
                entry.host.arch,
                entry.host.dflog_version
            ))
        )?;
        writeln!(
            out,
            "\tPrevious entry hash: {}",
            esc(entry.prev.as_deref().unwrap_or("N/A"))
        )?;

        writeln!(out, "\tKind: {}", entry.kind.name())?;
        match &entry.kind {
            EntryKind::Init { config } => {
                writeln!(out, "\t\tExaminer: {}", esc(&config.case.examiner))?;
                writeln!(out, "\t\tCase ID: {}", esc(&config.case.case_id))?;
                writeln!(out, "\t\tHash Algorithm: {}", config.hash.algorithm.name())?;
            }
            EntryKind::Run { command } => {
                writeln!(
                    out,
                    "\t\tCommand Line Arguments: {}",
                    esc(command.argv.join(" "))
                )?;
                writeln!(out, "\t\tWorking Directory: {}", esc(command.cwd.display()))?;
                match &command.program {
                    Some(program) => writeln!(
                        out,
                        "\t\tProgram: {} ({})",
                        esc(program.path.display()),
                        esc(digest(&program.digest))
                    )?,
                    None => writeln!(out, "\t\tProgram: N/A")?,
                }
                writeln!(out, "\t\tExit Status: {}", esc(&command.status))?;
                write_output(out, "Stdout", &command.stdout)?;
                write_output(out, "Stderr", &command.stderr)?;
            }
            // No kind-specific data: their changes are listed below.
            EntryKind::Record | EntryKind::Reindex => {}
        }

        // External changes happened before this entry started, so they
        // come before the entry's own changes.
        if !entry.external_changes.is_empty() {
            write_changes(out, &entry.external_changes, "External Changes:")?;
        }

        if !entry.changes.is_empty() {
            write_changes(out, &entry.changes, "Changes:")?;
        }

        Ok(())
    }
}

fn write_output(out: &mut dyn Write, name: &str, output: &CommandOutput) -> Result<()> {
    writeln!(
        out,
        "\t\t{}: {} ({} bytes, {})",
        name,
        esc(output.path.display()),
        output.size,
        esc(&output.hash)
    )?;
    Ok(())
}

fn write_changes(out: &mut dyn Write, changes: &[FileChange], title: &str) -> Result<()> {
    writeln!(out, "\t{}", title)?;

    for change in changes {
        writeln!(out, "\t\t{}: {}", esc(change.path.display()), change.change)?;
        match &change.change {
            Change::Created { after } | Change::Unchanged { after } => {
                writeln!(out, "\t\t\tAfter:  {}", stat(after))?;
            }
            Change::Modified { before, after } => {
                writeln!(out, "\t\t\tBefore: {}", stat(before))?;
                writeln!(out, "\t\t\tAfter:  {}", stat(after))?;
            }
            Change::Deleted { before } => {
                writeln!(out, "\t\t\tBefore: {}", stat(before))?;
            }
        }
    }
    Ok(())
}

fn stat(stat: &FileStat) -> String {
    format!("{} bytes, {}", stat.size, esc(digest(&stat.digest)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::Digest;
    use crate::report_renderer::test_fixtures::sample;

    fn render(report: &Report) -> String {
        let mut buf = Vec::new();
        TextRenderer.render(report, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn renders_all_entry_kinds() {
        let text = render(&sample(Verification::Valid {
            entries: 3,
            head: "sha256:head".into(),
        }));
        println!("{text}");

        assert!(
            text.contains("Log verification passed: 3 entries found, head hash is sha256:head")
        );
        assert!(text.contains("Entry 0\n\tMessage: init\n\tExaminer: Jane\n"));
        assert!(text.contains("\tKind: init\n\t\tExaminer: Jane\n"));
        assert!(text.contains("\tKind: run\n\t\tCommand Line Arguments: strings disk.img\n"));
        assert!(text.contains("\tKind: record\n\tExternal Changes:\n\t\tnotes.txt: modified\n\t\t\tBefore: 1 bytes, sha256:a\n"));
        assert!(text.contains("\tStarted: 2026-11-18 11:00:00 +01:00\n"));
        assert!(text.contains("\tPrevious entry hash: N/A\n"));
        assert!(text.contains("\t\tHash Algorithm: sha256\n"));
        assert!(text.contains("\t\tProgram: /usr/bin/strings (sha256:prog)\n"));
        assert!(text.contains("\t\tExit Status: exit 0\n"));
        assert!(text.contains("\t\tStdout: output/1.stdout (42 bytes, sha256:out)\n"));
        assert!(text.contains(
            "\tChanges:\n\t\tstrings.txt: created\n\t\t\tAfter:  42 bytes, sha256:new\n"
        ));
        assert!(text.contains("\t\tdisk.img: unchanged\n"));
        assert!(text.contains("\tExternal Changes:\n\t\tnotes.txt: modified\n"));
        assert!(text.contains("\t\t\tAfter:  2 bytes, not hashed: permission denied\n"));
        assert!(text.contains("Entry 3\n\tMessage: index lost\n"));
        assert!(text.contains(
            "\tKind: reindex\n\tExternal Changes:\n\t\tlost.txt: deleted\n\t\t\tBefore: 7 bytes, sha256:gone\n"
        ));
    }

    #[test]
    fn renders_broken_chain_warning() {
        let text = render(&sample(Verification::Broken {
            line: 2,
            seq: None,
            reason: "bad json".into(),
        }));
        assert!(text.contains("Log verification failed: 'bad json' at line 2 (sequence N/A)"));
        assert!(text.contains("may not be valid"));
    }

    /// Values a hostile file name or a tampered journal could control must
    /// not be able to start a new line or reach the terminal raw.
    #[test]
    fn journal_text_cannot_forge_lines_or_escape_sequences() {
        let evil = "evil\n\t\tfake.txt: deleted\u{1b}[2J";
        let mut report = sample(Verification::Broken {
            line: 1,
            seq: None,
            reason: evil.into(),
        });
        report.case_id = evil.into();
        report.examiner = evil.into();
        for entry in report.entries.iter_mut().flatten() {
            entry.message = evil.into();
            entry.examiner = evil.into();
            entry.host.hostname = evil.into();
            entry.prev = Some(evil.into());
            match &mut entry.kind {
                EntryKind::Init { config } => {
                    config.case.examiner = evil.into();
                    config.case.case_id = evil.into();
                }
                EntryKind::Run { command } => {
                    command.argv = vec![evil.into()];
                    command.cwd = evil.into();
                    command.status = crate::journal::ExitStatus::SpawnFailed(evil.into());
                    command.stdout.path = evil.into();
                    command.stdout.hash = evil.into();
                    if let Some(program) = &mut command.program {
                        program.path = evil.into();
                        program.digest = Digest::Error(evil.into());
                    }
                }
                EntryKind::Record | EntryKind::Reindex => {}
            }
            for change in entry.changes.iter_mut().chain(&mut entry.external_changes) {
                change.path = evil.into();
                change.change = Change::Created {
                    after: FileStat {
                        size: 1,
                        digest: Digest::Symlink(evil.into()),
                    },
                };
            }
        }

        let text = render(&report);
        assert!(!text.contains('\u{1b}'), "raw ESC in output:\n{text}");
        assert!(
            !text.contains("\nfake.txt"),
            "forged line in output:\n{text}"
        );
        assert!(
            !text.contains("\n\t\tfake.txt"),
            "forged line in output:\n{text}"
        );
        assert!(text.contains("evil\\n\\t\\tfake.txt: deleted\\u{1b}[2J"));
    }
}
