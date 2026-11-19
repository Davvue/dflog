//! HTML report: a single self-contained file with inline CSS and no scripts
//! or external resources, so it opens offline and can be archived or
//! printed as-is.
//!
//! Everything taken from the journal (messages, paths, argv, error text) is
//! untrusted and goes through [`esc`]. A file named `<script>.txt` in the
//! working directory must show up as text, not run in the reader's browser.

use crate::error::Result;
use crate::journal::{Change, CommandOutput, Entry, EntryKind, FileChange, FileStat, Verification};
use crate::report_renderer::format::{digest, local_time};
use crate::report_renderer::{Renderer, Report};
use std::fmt::{self, Display, Formatter};
use std::io::Write;

/// Standalone HTML page, meant to be read in a browser or printed.
pub struct HtmlRenderer;

impl Renderer for HtmlRenderer {
    fn render(&self, report: &Report, out: &mut dyn Write) -> Result<()> {
        writeln!(out, "<!doctype html>")?;
        writeln!(out, "<html lang=\"en\">")?;
        writeln!(out, "<head>")?;
        writeln!(out, "<meta charset=\"utf-8\">")?;
        writeln!(
            out,
            "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">"
        )?;
        writeln!(
            out,
            "<meta name=\"generator\" content=\"dflog {}\">",
            esc(report.dflog_version)
        )?;
        writeln!(out, "<title>Case {} report</title>", esc(&report.case_id))?;
        writeln!(out, "<style>{STYLE}</style>")?;
        writeln!(out, "</head>")?;
        writeln!(out, "<body>")?;
        writeln!(out, "<main>")?;

        writeln!(out, "<header>")?;
        writeln!(out, "<h1>Case {}</h1>", esc(&report.case_id))?;
        writeln!(out, "<dl>")?;
        row(out, "Examiner", &report.examiner)?;
        row(out, "Generated", report.generated_at)?;
        row(out, "dflog version", report.dflog_version)?;
        row_code(out, "Journal file hash", &report.journal_hash)?;
        writeln!(out, "</dl>")?;
        writeln!(out, "</header>")?;

        match &report.verification {
            Verification::Valid { entries, head } => {
                writeln!(out, "<section class=\"verification ok\" role=\"status\">")?;
                writeln!(
                    out,
                    "<strong>Journal verification passed.</strong> \
                     {} entries found, head hash is <code>{}</code>.",
                    entries,
                    esc(head)
                )?;
            }
            Verification::Broken { line, seq, reason } => {
                writeln!(
                    out,
                    "<section class=\"verification broken\" role=\"alert\">"
                )?;
                writeln!(
                    out,
                    "<strong>Journal verification failed</strong> at line {} (sequence {}): {}",
                    line,
                    seq.map_or("N/A".into(), |s| s.to_string()),
                    esc(reason)
                )?;
                writeln!(
                    out,
                    "<p>The following report may not be valid. Proceed with caution.</p>"
                )?;
            }
        }
        writeln!(out, "</section>")?;

        writeln!(out, "<section>")?;
        writeln!(out, "<h2>Journal entries</h2>")?;
        if report.entries.is_empty() {
            writeln!(out, "<p>No entries.</p>")?;
        }
        for entry in &report.entries {
            match entry {
                Ok(entry) => self.render_entry(entry, out)?,
                Err(e) => {
                    writeln!(out, "<article class=\"entry invalid\">")?;
                    writeln!(out, "<h3>Invalid entry</h3>")?;
                    writeln!(out, "<p>{}</p>", esc(e))?;
                    writeln!(out, "</article>")?;
                }
            }
        }
        writeln!(out, "</section>")?;

        writeln!(out, "</main>")?;
        writeln!(out, "</body>")?;
        writeln!(out, "</html>")?;
        Ok(())
    }
}

impl HtmlRenderer {
    /// Writes one journal entry as an `<article id="entry-{seq}">`.
    fn render_entry(&self, entry: &Entry, out: &mut dyn Write) -> Result<()> {
        let offset = entry.host.utc_offset_seconds;
        let kind = entry.kind.name();

        writeln!(out, "<article class=\"entry\" id=\"entry-{}\">", entry.seq)?;
        writeln!(
            out,
            "<h3><span class=\"seq\">#{}</span> <span class=\"kind\">{}</span> {}</h3>",
            entry.seq,
            esc(kind),
            esc(&entry.message)
        )?;

        writeln!(out, "<dl>")?;
        row(out, "Examiner", &entry.examiner)?;
        row(out, "Started", local_time(entry.started_at, offset))?;
        row(out, "Finished", local_time(entry.finished_at, offset))?;
        row(
            out,
            "Host",
            format_args!(
                "{} (user {}, {}/{}, dflog {})",
                entry.host.hostname,
                entry.host.user,
                entry.host.os,
                entry.host.arch,
                entry.host.dflog_version
            ),
        )?;
        match &entry.prev {
            // The previous entry is in this report, so link to it.
            Some(prev) if entry.seq > 0 => writeln!(
                out,
                "<dt>Previous entry hash</dt><dd><a href=\"#entry-{}\"><code>{}</code></a></dd>",
                entry.seq - 1,
                esc(prev)
            )?,
            Some(prev) => row_code(out, "Previous entry hash", prev)?,
            None => row(out, "Previous entry hash", "N/A")?,
        }

        writeln!(out, "</dl>")?;

        // Same grouping as the text report: entry details, then the
        // kind-specific details, then each list of changes.
        match &entry.kind {
            EntryKind::Init { config } => {
                group_start(out, "Case configuration")?;
                writeln!(out, "<dl class=\"nested\">")?;
                row(out, "Examiner", &config.case.examiner)?;
                row(out, "Case ID", &config.case.case_id)?;
                row(out, "Hash algorithm", config.hash.algorithm.name())?;
                writeln!(out, "</dl>")?;
                group_end(out)?;
            }
            EntryKind::Run { command } => {
                group_start(out, "Command")?;
                writeln!(out, "<dl class=\"nested\">")?;
                row_code(out, "Command line", command.argv.join(" "))?;
                row_code(out, "Working directory", command.cwd.display())?;
                match &command.program {
                    Some(program) => row_code(
                        out,
                        "Program",
                        format_args!("{} ({})", program.path.display(), digest(&program.digest)),
                    )?,
                    None => row(out, "Program", "N/A")?,
                }
                row(out, "Exit status", &command.status)?;
                output_row(out, "Stdout", &command.stdout)?;
                output_row(out, "Stderr", &command.stderr)?;
                writeln!(out, "</dl>")?;
                group_end(out)?;
            }
            EntryKind::Record => {}
        }

        // External changes happened before this entry started, so they
        // come before the entry's own changes.
        if !entry.external_changes.is_empty() {
            changes_table(out, "External changes", &entry.external_changes)?;
        }
        if !entry.changes.is_empty() {
            changes_table(out, "Changes", &entry.changes)?;
        }

        writeln!(out, "</article>")?;
        Ok(())
    }
}

/// Opens a titled group inside an entry, set off by a divider line.
fn group_start(out: &mut dyn Write, title: &str) -> Result<()> {
    writeln!(out, "<section class=\"group\">")?;
    writeln!(out, "<h4>{}</h4>", title)?;
    Ok(())
}

fn group_end(out: &mut dyn Write) -> Result<()> {
    writeln!(out, "</section>")?;
    Ok(())
}

/// `<dt>label</dt><dd>value</dd>`, with `value` escaped.
fn row(out: &mut dyn Write, label: &str, value: impl Display) -> Result<()> {
    writeln!(out, "<dt>{}</dt><dd>{}</dd>", label, esc(value))?;
    Ok(())
}

/// Like [`row`], but shows the value as code (hashes, paths, argv).
fn row_code(out: &mut dyn Write, label: &str, value: impl Display) -> Result<()> {
    writeln!(
        out,
        "<dt>{}</dt><dd><code>{}</code></dd>",
        label,
        esc(value)
    )?;
    Ok(())
}

fn output_row(out: &mut dyn Write, name: &str, output: &CommandOutput) -> Result<()> {
    writeln!(
        out,
        "<dt>{}</dt><dd><code>{}</code> ({} bytes, <code>{}</code>)</dd>",
        name,
        esc(output.path.display()),
        output.size,
        esc(&output.hash)
    )?;
    Ok(())
}

fn changes_table(out: &mut dyn Write, title: &str, changes: &[FileChange]) -> Result<()> {
    group_start(out, title)?;
    writeln!(out, "<div class=\"table-wrap\"><table>")?;
    writeln!(
        out,
        "<thead><tr><th>Path</th><th>Change</th><th>Before</th><th>After</th></tr></thead>"
    )?;
    writeln!(out, "<tbody>")?;
    for change in changes {
        let (class, before, after) = match &change.change {
            Change::Created { after } => ("created", None, Some(after)),
            Change::Unchanged { after } => ("unchanged", None, Some(after)),
            Change::Modified { before, after } => ("modified", Some(before), Some(after)),
            Change::Deleted { before } => ("deleted", Some(before), None),
        };
        writeln!(
            out,
            "<tr class=\"{}\"><td><code>{}</code></td><td>{}</td><td>{}</td><td>{}</td></tr>",
            class,
            esc(change.path.display()),
            esc(&change.change),
            StatCell(before),
            StatCell(after)
        )?;
    }
    writeln!(out, "</tbody>")?;
    writeln!(out, "</table></div>")?;
    group_end(out)
}

/// Size and digest of one side of a change, or a dash if that side does
/// not exist (before a creation, after a deletion).
struct StatCell<'a>(Option<&'a FileStat>);

impl Display for StatCell<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(stat) => write!(
                f,
                "{} bytes<br><code>{}</code>",
                stat.size,
                esc(digest(&stat.digest))
            ),
            None => f.write_str("&mdash;"),
        }
    }
}

/// Wraps a value so that its `Display` output is HTML-escaped. Safe for
/// element content and double- or single-quoted attribute values.
fn esc<T: Display>(value: T) -> Escaped<T> {
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
                let mut rest = s;
                while let Some(i) = rest.find(['&', '<', '>', '"', '\'']) {
                    self.0.write_str(&rest[..i])?;
                    self.0.write_str(match rest.as_bytes()[i] {
                        b'&' => "&amp;",
                        b'<' => "&lt;",
                        b'>' => "&gt;",
                        b'"' => "&quot;",
                        _ => "&#39;",
                    })?;
                    rest = &rest[i + 1..];
                }
                self.0.write_str(rest)
            }
        }

        write!(EscapeWriter(f), "{}", self.0)
    }
}

/// Light and dark color schemes follow the reader's system; printing
/// always uses the light one.
const STYLE: &str = r#"
:root {
  color-scheme: light dark;
  --bg: #ffffff; --fg: #1f2328; --muted: #59636e; --border: #d1d9e0;
  --surface: #f6f8fa; --accent: #0969da;
  --ok-bg: #dafbe1; --ok-fg: #116329;
  --warn-fg: #9a6700;
  --bad-bg: #ffebe9; --bad-fg: #a40e26;
}
@media (prefers-color-scheme: dark) {
  :root {
    --bg: #0d1117; --fg: #e6edf3; --muted: #9198a1; --border: #3d444d;
    --surface: #151b23; --accent: #4493f8;
    --ok-bg: #12261e; --ok-fg: #57ab5a;
    --warn-fg: #d29922;
    --bad-bg: #2d1214; --bad-fg: #f47067;
  }
}
* { box-sizing: border-box; }
body {
  margin: 0; background: var(--bg); color: var(--fg);
  font: 15px/1.5 system-ui, -apple-system, "Segoe UI", sans-serif;
}
main { max-width: 64rem; margin: 0 auto; padding: 2rem 1rem; }
h1 { margin: 0 0 1rem; font-size: 1.6rem; }
h2 { margin: 2rem 0 1rem; font-size: 1.25rem; }
h3 { margin: 0 0 .75rem; font-size: 1.05rem; }
h4 {
  margin: 0 0 .5rem; font-size: .8rem; font-weight: 600; color: var(--muted);
  text-transform: uppercase; letter-spacing: .04em;
}
.group { margin-top: .9rem; padding-top: .75rem; border-top: 1px solid var(--border); }
dl.nested { padding-left: 1rem; }
code {
  font: 13px/1.4 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  overflow-wrap: anywhere;
}
a { color: var(--accent); }
dl { display: grid; grid-template-columns: max-content 1fr; gap: .25rem 1rem; margin: 0; }
dt { color: var(--muted); }
dd { margin: 0; min-width: 0; }
.verification {
  margin: 1.5rem 0; padding: .75rem 1rem;
  border: 1px solid currentColor; border-radius: 6px;
}
.verification p { margin: .5rem 0 0; }
.ok { background: var(--ok-bg); color: var(--ok-fg); }
.broken { background: var(--bad-bg); color: var(--bad-fg); }
.entry {
  margin: 0 0 1rem; padding: 1rem;
  border: 1px solid var(--border); border-radius: 6px;
  break-inside: avoid;
}
.entry.invalid { border-color: var(--bad-fg); background: var(--bad-bg); }
.seq { color: var(--muted); font-weight: normal; }
.kind {
  display: inline-block; padding: 0 .4rem; vertical-align: middle;
  background: var(--surface); border: 1px solid var(--border); border-radius: 4px;
  font-size: .8rem; font-weight: 600;
}
.table-wrap { overflow-x: auto; }
table { width: 100%; border-collapse: collapse; font-size: .9rem; }
th, td {
  padding: .35rem .5rem; text-align: left; vertical-align: top;
  border-top: 1px solid var(--border);
}
th { color: var(--muted); font-weight: 600; }
tr.created td:nth-child(2) { color: var(--ok-fg); }
tr.modified td:nth-child(2) { color: var(--warn-fg); }
tr.deleted td:nth-child(2) { color: var(--bad-fg); }
tr.unchanged { color: var(--muted); }
@media (max-width: 40rem) {
  dl { grid-template-columns: 1fr; gap: 0; }
  dt { margin-top: .4rem; }
}
@media print {
  :root {
    color-scheme: light;
    --bg: #ffffff; --fg: #000000; --muted: #444444; --border: #bbbbbb;
    --surface: #f2f2f2; --accent: #000000;
    --ok-bg: #ffffff; --ok-fg: #116329;
    --warn-fg: #7a5200;
    --bad-bg: #ffffff; --bad-fg: #a40e26;
  }
  body { font-size: 10pt; }
  main { max-width: none; padding: 0; }
  a { text-decoration: none; }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use crate::report_renderer::test_fixtures::sample;

    fn render(report: &Report) -> String {
        let mut buf = Vec::new();
        HtmlRenderer.render(report, &mut buf).unwrap();
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
        let html = render(&sample(valid()));

        assert!(html.starts_with("<!doctype html>\n"));
        assert!(html.trim_end().ends_with("</html>"));
        assert!(html.contains("<title>Case CASE-1 report</title>"));
        assert!(html.contains("<section class=\"verification ok\" role=\"status\">"));
        assert!(html.contains("head hash is <code>sha256:head</code>"));
        assert!(html.contains("<dt>Journal file hash</dt><dd><code>sha256:journal</code></dd>"));

        for seq in 0..3 {
            assert!(html.contains(&format!("<article class=\"entry\" id=\"entry-{seq}\">")));
        }
        assert!(html.contains("<span class=\"kind\">run</span> extract strings</h3>"));
        assert!(html.contains("<dt>Started</dt><dd>2026-11-18 11:00:00 +01:00</dd>"));
        assert!(html.contains("<dt>Previous entry hash</dt><dd>N/A</dd>"));
        assert!(html.contains("<a href=\"#entry-0\"><code>sha256:prev1</code></a>"));
        assert!(html.contains("<dt>Hash algorithm</dt><dd>sha256</dd>"));
        assert!(html.contains("<dt>Command line</dt><dd><code>strings disk.img</code></dd>"));
        assert!(html.contains("<code>/usr/bin/strings (sha256:prog)</code>"));
        assert!(html.contains("<dt>Exit status</dt><dd>exit 0</dd>"));
        assert!(html.contains(
            "<dt>Stdout</dt><dd><code>output/1.stdout</code> (42 bytes, <code>sha256:out</code>)</dd>"
        ));
        assert!(html.contains(
            "<tr class=\"created\"><td><code>strings.txt</code></td><td>created</td>\
             <td>&mdash;</td><td>42 bytes<br><code>sha256:new</code></td></tr>"
        ));
        assert!(html.contains("<tr class=\"unchanged\"><td><code>disk.img</code></td>"));
        assert!(html.contains("<code>not hashed: permission denied</code>"));

        // Grouped like the text report: entry details, kind details, changes.
        let run = &html[html.find("id=\"entry-1\"").unwrap()..html.find("id=\"entry-2\"").unwrap()];
        let details = run.find("<dt>Previous entry hash</dt>").unwrap();
        let command = run
            .find("<section class=\"group\">\n<h4>Command</h4>")
            .unwrap();
        let changes = run
            .find("<section class=\"group\">\n<h4>Changes</h4>")
            .unwrap();
        assert!(details < command && command < changes);
        assert!(html.contains("<h4>Case configuration</h4>"));

        // External changes come before the entry's own changes.
        let record = html.find("id=\"entry-2\"").unwrap();
        assert!(html[record..].contains("<h4>External changes</h4>"));
    }

    #[test]
    fn escapes_journal_text() {
        let mut report = sample(valid());
        report.case_id = "<b>case</b>".into();
        let Ok(run) = &mut report.entries[1] else {
            unreachable!()
        };
        run.message = "<script>alert('x')</script>".into();
        run.changes[0].path = "a\"&<img src=x>.txt".into();

        let html = render(&report);

        assert!(!html.contains("<script>"));
        assert!(!html.contains("<img"));
        assert!(!html.contains("<b>case"));
        assert!(html.contains("&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt;"));
        assert!(html.contains("<code>a&quot;&amp;&lt;img src=x&gt;.txt</code>"));
        assert!(html.contains("<title>Case &lt;b&gt;case&lt;/b&gt; report</title>"));
    }

    #[test]
    fn renders_broken_chain_warning() {
        let html = render(&sample(Verification::Broken {
            line: 2,
            seq: None,
            reason: "bad <json>".into(),
        }));
        assert!(html.contains("<section class=\"verification broken\" role=\"alert\">"));
        assert!(html.contains("at line 2 (sequence N/A): bad &lt;json&gt;"));
        assert!(html.contains("Proceed with caution"));
    }

    #[test]
    fn renders_invalid_entry() {
        let mut report = sample(valid());
        report
            .entries
            .push(Err(Error::JournalCorrupt("line 4: <garbage>".into())));

        let html = render(&report);

        assert!(html.contains(
            "<article class=\"entry invalid\">\n<h3>Invalid entry</h3>\n\
             <p>journal is corrupt: line 4: &lt;garbage&gt;</p>"
        ));
    }

    #[test]
    fn renders_empty_journal() {
        let mut report = sample(valid());
        report.entries.clear();
        assert!(render(&report).contains("<p>No entries.</p>"));
    }
}
