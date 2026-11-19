//! Markdown report (GitHub-flavored, for the tables), meant for case notes
//! kept in a wiki or a repository.
//!
//! Text from the journal is untrusted. Most Markdown viewers also render
//! raw HTML and links, so plain text goes through [`text`], which escapes
//! Markdown syntax, and paths, hashes and argv go into code spans through
//! [`code`], which shows its content literally.

use crate::error::Result;
use crate::journal::{Change, CommandOutput, Entry, EntryKind, FileChange, FileStat, Verification};
use crate::report_renderer::format::{digest, local_time};
use crate::report_renderer::{Renderer, Report};
use std::fmt::{self, Display, Formatter};
use std::io::Write;

/// Markdown document with the same layout as the HTML report.
pub struct MarkdownRenderer;

impl Renderer for MarkdownRenderer {
    fn render(&self, report: &Report, out: &mut dyn Write) -> Result<()> {
        writeln!(out, "# Case {}", text(&report.case_id))?;
        writeln!(out)?;
        row(out, "Examiner", text(&report.examiner))?;
        row(out, "Generated", report.generated_at)?;
        row(out, "dflog version", text(report.dflog_version))?;
        row(out, "Journal file hash", code(&report.journal_hash))?;
        writeln!(out)?;

        match &report.verification {
            Verification::Valid { entries, head } => {
                writeln!(
                    out,
                    "> **Journal verification passed.** {} entries found, head hash is {}.",
                    entries,
                    code(head)
                )?;
            }
            Verification::Broken { line, seq, reason } => {
                writeln!(
                    out,
                    "> **Journal verification failed** at line {} (sequence {}): {}",
                    line,
                    seq.map_or("N/A".into(), |s| s.to_string()),
                    text(reason)
                )?;
                writeln!(out, ">")?;
                writeln!(
                    out,
                    "> The following report may not be valid. Proceed with caution."
                )?;
            }
        }
        writeln!(out)?;

        writeln!(out, "## Journal entries")?;
        if report.entries.is_empty() {
            writeln!(out)?;
            writeln!(out, "No entries.")?;
        }
        for (i, entry) in report.entries.iter().enumerate() {
            writeln!(out)?;
            if i > 0 {
                writeln!(out, "---")?;
                writeln!(out)?;
            }
            match entry {
                Ok(entry) => self.render_entry(entry, out)?,
                Err(e) => {
                    writeln!(out, "### Invalid entry")?;
                    writeln!(out)?;
                    writeln!(out, "{}", text(e))?;
                }
            }
        }

        Ok(())
    }
}

impl MarkdownRenderer {
    /// Writes one journal entry, starting with its `###` heading.
    fn render_entry(&self, entry: &Entry, out: &mut dyn Write) -> Result<()> {
        let offset = entry.host.utc_offset_seconds;

        writeln!(
            out,
            "### #{} · {} · {}",
            entry.seq,
            entry.kind.name(),
            text(&entry.message)
        )?;
        writeln!(out)?;
        row(out, "Examiner", text(&entry.examiner))?;
        row(out, "Started", local_time(entry.started_at, offset))?;
        row(out, "Finished", local_time(entry.finished_at, offset))?;
        row(
            out,
            "Host",
            text(format_args!(
                "{} (user {}, {}/{}, dflog {})",
                entry.host.hostname,
                entry.host.user,
                entry.host.os,
                entry.host.arch,
                entry.host.dflog_version
            )),
        )?;
        match &entry.prev {
            Some(prev) => row(out, "Previous entry hash", code(prev))?,
            None => row(out, "Previous entry hash", "N/A")?,
        }

        // Same grouping as the text report: entry details, then the
        // kind-specific details, then each list of changes.
        match &entry.kind {
            EntryKind::Init { config } => {
                group(out, "Case configuration")?;
                row(out, "Examiner", text(&config.case.examiner))?;
                row(out, "Case ID", text(&config.case.case_id))?;
                row(out, "Hash algorithm", config.hash.algorithm.name())?;
            }
            EntryKind::Run { command } => {
                group(out, "Command")?;
                row(out, "Command line", code(command.argv.join(" ")))?;
                row(out, "Working directory", code(command.cwd.display()))?;
                match &command.program {
                    Some(program) => row(
                        out,
                        "Program",
                        format_args!(
                            "{} ({})",
                            code(program.path.display()),
                            code(digest(&program.digest))
                        ),
                    )?,
                    None => row(out, "Program", "N/A")?,
                }
                row(out, "Exit status", text(&command.status))?;
                output_row(out, "Stdout", &command.stdout)?;
                output_row(out, "Stderr", &command.stderr)?;
            }
            // No kind-specific data: their changes are listed below.
            EntryKind::Record | EntryKind::Reindex => {}
        }

        // External changes happened before this entry started, so they
        // come before the entry's own changes.
        if !entry.external_changes.is_empty() {
            changes_table(out, "External changes", &entry.external_changes)?;
        }
        if !entry.changes.is_empty() {
            changes_table(out, "Changes", &entry.changes)?;
        }

        Ok(())
    }
}

/// Starts a titled group inside an entry.
fn group(out: &mut dyn Write, title: &str) -> Result<()> {
    writeln!(out)?;
    writeln!(out, "#### {}", title)?;
    writeln!(out)?;
    Ok(())
}

/// `- **label:** value`. `value` must already be escaped or a code span.
fn row(out: &mut dyn Write, label: &str, value: impl Display) -> Result<()> {
    writeln!(out, "- **{}:** {}", label, value)?;
    Ok(())
}

fn output_row(out: &mut dyn Write, name: &str, output: &CommandOutput) -> Result<()> {
    row(
        out,
        name,
        format_args!(
            "{} ({} bytes, {})",
            code(output.path.display()),
            output.size,
            code(&output.hash)
        ),
    )
}

fn changes_table(out: &mut dyn Write, title: &str, changes: &[FileChange]) -> Result<()> {
    group(out, title)?;
    writeln!(out, "| Path | Change | Before | After |")?;
    writeln!(out, "| --- | --- | --- | --- |")?;
    for change in changes {
        let (before, after) = match &change.change {
            Change::Created { after } | Change::Unchanged { after } => (None, Some(after)),
            Change::Modified { before, after } => (Some(before), Some(after)),
            Change::Deleted { before } => (Some(before), None),
        };
        writeln!(
            out,
            "| {} | {} | {} | {} |",
            table_code(change.path.display()),
            text(&change.change),
            stat_cell(before),
            stat_cell(after)
        )?;
    }
    Ok(())
}

/// Size and digest of one side of a change, or a dash if that side does
/// not exist (before a creation, after a deletion).
fn stat_cell(stat: Option<&FileStat>) -> String {
    match stat {
        Some(stat) => format!("{} bytes, {}", stat.size, table_code(digest(&stat.digest))),
        None => "—".into(),
    }
}

/// Escapes `value` for inline Markdown text. Characters that could start
/// emphasis, links, HTML, entities, headings or table cells are
/// backslash-escaped, and line breaks become spaces so a value can't
/// start a new block.
fn text<T: Display>(value: T) -> Escaped<T> {
    Escaped(value)
}

struct Escaped<T>(T);

impl<T: Display> Display for Escaped<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        use fmt::Write as _;

        /// Escapes each chunk the inner `Display` writes, so nothing is
        /// buffered.
        struct EscapeWriter<'a, 'b>(&'a mut Formatter<'b>);

        impl fmt::Write for EscapeWriter<'_, '_> {
            fn write_str(&mut self, s: &str) -> fmt::Result {
                for c in s.chars() {
                    match c {
                        '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '#' | '|' | '~' | '&'
                        | '!' => {
                            self.0.write_char('\\')?;
                            self.0.write_char(c)?;
                        }
                        '\n' | '\r' => self.0.write_char(' ')?,
                        _ => self.0.write_char(c)?,
                    }
                }
                Ok(())
            }
        }

        write!(EscapeWriter(f), "{}", self.0)
    }
}

/// A code span showing `value` literally. The fence is one backtick longer
/// than the longest run of backticks in `value`, so `value` can't close it.
fn code(value: impl Display) -> String {
    code_span(value.to_string())
}

/// Like [`code`], for table cells: GitHub-flavored Markdown splits cells at
/// `|` even inside code spans, so pipes are escaped as `\|`.
fn table_code(value: impl Display) -> String {
    code_span(value.to_string().replace('|', "\\|"))
}

fn code_span(value: String) -> String {
    let value = value.replace(['\n', '\r'], " ");
    let mut longest = 0;
    let mut run = 0;
    for c in value.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let fence = "`".repeat(longest + 1);
    // A space on both sides is stripped again when rendering. It keeps a
    // leading or trailing backtick in `value` from merging with the fence.
    let pad = if value.starts_with('`') || value.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{value}{pad}{fence}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use crate::report_renderer::test_fixtures::sample;

    fn render(report: &Report) -> String {
        let mut buf = Vec::new();
        MarkdownRenderer.render(report, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    fn valid() -> Verification {
        Verification::Valid {
            entries: 3,
            head: "sha256:head".into(),
        }
    }

    #[test]
    fn renders_all_entry_kinds() {
        let md = render(&sample(valid()));

        assert!(md.starts_with("# Case CASE-1\n"));
        assert!(md.contains("- **Journal file hash:** `sha256:journal`\n"));
        assert!(md.contains(
            "> **Journal verification passed.** 3 entries found, head hash is `sha256:head`.\n"
        ));
        assert!(md.contains("### #0 · init · init\n"));
        assert!(md.contains("### #1 · run · extract strings\n"));
        assert!(md.contains("- **Started:** 2026-11-18 11:00:00 +01:00\n"));
        assert!(md.contains("- **Previous entry hash:** N/A\n"));
        assert!(md.contains("- **Previous entry hash:** `sha256:prev1`\n"));
        assert!(md.contains("#### Case configuration\n\n- **Examiner:** Jane\n"));
        assert!(md.contains("- **Hash algorithm:** sha256\n"));
        assert!(md.contains("#### Command\n\n- **Command line:** `strings disk.img`\n"));
        assert!(md.contains("- **Program:** `/usr/bin/strings` (`sha256:prog`)\n"));
        assert!(md.contains("- **Exit status:** exit 0\n"));
        assert!(md.contains("- **Stdout:** `output/1.stdout` (42 bytes, `sha256:out`)\n"));
        assert!(md.contains(
            "#### Changes\n\n| Path | Change | Before | After |\n| --- | --- | --- | --- |\n\
             | `strings.txt` | created | — | 42 bytes, `sha256:new` |\n"
        ));
        assert!(md.contains("| `disk.img` | unchanged | — | 1024 bytes, `sha256:img` |\n"));
        assert!(md.contains(
            "| `notes.txt` | modified | 1 bytes, `sha256:a` | 2 bytes, `not hashed: permission denied` |\n"
        ));

        // Entries are separated by rules, with a blank line before each so
        // `---` is never read as a heading underline.
        assert_eq!(md.matches("\n\n---\n\n### ").count(), 3);

        // Grouped like the text report: entry details, kind details, changes.
        let run = &md[md.find("### #1").unwrap()..md.find("### #2").unwrap()];
        let details = run.find("- **Previous entry hash:**").unwrap();
        let command = run.find("#### Command").unwrap();
        let changes = run.find("#### Changes").unwrap();
        assert!(details < command && command < changes);

        let record = &md[md.find("### #2").unwrap()..md.find("### #3").unwrap()];
        assert!(record.contains("#### External changes\n"));

        let reindex = &md[md.find("### #3 · reindex · index lost\n").unwrap()..];
        assert!(reindex.contains(
            "#### External changes\n\n| Path | Change | Before | After |\n| --- | --- | --- | --- |\n\
             | `lost.txt` | deleted | 7 bytes, `sha256:gone` | — |\n"
        ));
        assert!(!reindex.contains("#### Command"));
    }

    #[test]
    fn escapes_journal_text() {
        let mut report = sample(valid());
        let Ok(run) = &mut report.entries[1] else {
            unreachable!()
        };
        run.message = "*bold* [link](javascript:x) <script>\n# heading".into();
        run.changes[0].path = "a|b`c.txt".into();
        if let EntryKind::Run { command } = &mut run.kind {
            command.argv = vec!["echo".into(), "`hi`".into()];
        }

        let md = render(&report);

        assert!(md.contains(
            "### #1 · run · \\*bold\\* \\[link\\](javascript:x) \\<script\\> \\# heading\n"
        ));
        assert!(md.contains("| ``a\\|b`c.txt`` | created |"));
        assert!(md.contains("- **Command line:** `` echo `hi` ``\n"));
    }

    #[test]
    fn code_span_fences() {
        assert_eq!(code("plain"), "`plain`");
        assert_eq!(code("a`b"), "``a`b``");
        assert_eq!(code("``x"), "``` ``x ```");
        assert_eq!(code("line\nbreak"), "`line break`");
        assert_eq!(table_code("a|b"), "`a\\|b`");
    }

    #[test]
    fn renders_broken_chain_warning() {
        let md = render(&sample(Verification::Broken {
            line: 2,
            seq: None,
            reason: "bad <json>".into(),
        }));
        assert!(md.contains(
            "> **Journal verification failed** at line 2 (sequence N/A): bad \\<json\\>\n>\n\
             > The following report may not be valid. Proceed with caution.\n"
        ));
    }

    #[test]
    fn renders_invalid_entry() {
        let mut report = sample(valid());
        report
            .entries
            .push(Err(Error::JournalCorrupt("line 4: <garbage>".into())));
        assert!(
            render(&report)
                .contains("### Invalid entry\n\njournal is corrupt: line 4: \\<garbage\\>\n")
        );
    }

    #[test]
    fn renders_empty_journal() {
        let mut report = sample(valid());
        report.entries.clear();
        assert!(render(&report).ends_with("## Journal entries\n\nNo entries.\n"));
    }
}
