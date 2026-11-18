use crate::error::Result;
use crate::journal::{
    Change, CommandOutput, Digest, Entry, EntryKind, FileChange, FileStat, Verification,
};
use crate::report_renderer::{Renderer, Report};
use std::io::Write;

/// Plain-text report, meant to be read by people and attached to case notes.
pub struct TextRenderer;

impl Renderer for TextRenderer {
    fn render(&self, report: &Report, out: &mut dyn Write) -> Result<()> {
        writeln!(
            out,
            "Report for case '{}' by examiner '{}'",
            report.case_id, report.examiner
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
                    reason,
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
                Err(e) => writeln!(out, "invalid entry: {}", e)?,
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
        writeln!(out, "\tMessage: {}", entry.message)?;
        writeln!(out, "\tExaminer: {}", entry.examiner)?;
        writeln!(out, "\tStarted: {}", local_time(entry.started_at, offset))?;
        writeln!(out, "\tFinished: {}", local_time(entry.finished_at, offset))?;
        writeln!(
            out,
            "\tHost: {} (user {}, {}/{}, dflog {})",
            entry.host.hostname,
            entry.host.user,
            entry.host.os,
            entry.host.arch,
            entry.host.dflog_version
        )?;
        writeln!(
            out,
            "\tPrevious entry hash: {}",
            entry.prev.as_deref().unwrap_or("N/A")
        )?;

        writeln!(out, "\tKind: {}", entry.kind.name())?;
        match &entry.kind {
            EntryKind::Init { config } => {
                writeln!(out, "\t\tExaminer: {}", config.case.examiner)?;
                writeln!(out, "\t\tCase ID: {}", config.case.case_id)?;
                writeln!(out, "\t\tHash Algorithm: {}", config.hash.algorithm.name())?;
            }
            EntryKind::Run { command } => {
                writeln!(
                    out,
                    "\t\tCommand Line Arguments: {}",
                    command.argv.join(" ")
                )?;
                writeln!(out, "\t\tWorking Directory: {}", command.cwd.display())?;
                match &command.program {
                    Some(program) => writeln!(
                        out,
                        "\t\tProgram: {} ({})",
                        program.path.display(),
                        digest(&program.digest)
                    )?,
                    None => writeln!(out, "\t\tProgram: N/A")?,
                }
                writeln!(out, "\t\tExit Status: {}", command.status)?;
                write_output(out, "Stdout", &command.stdout)?;
                write_output(out, "Stderr", &command.stderr)?;
            }
            EntryKind::Record => {}
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
        output.path.display(),
        output.size,
        output.hash
    )?;
    Ok(())
}

fn write_changes(out: &mut dyn Write, changes: &[FileChange], title: &str) -> Result<()> {
    writeln!(out, "\t{}", title)?;

    for change in changes {
        writeln!(out, "\t\t{}: {}", change.path.display(), change.change)?;
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
    format!("{} bytes, {}", stat.size, digest(&stat.digest))
}

fn digest(digest: &Digest) -> String {
    match digest {
        Digest::Hash(hash) => hash.clone(),
        Digest::Symlink(target) => format!("symlink to {}", target.display()),
        Digest::Error(msg) => format!("not hashed: {}", msg),
    }
}

/// Formats `ts` in the local time of the host that wrote the entry. Falls
/// back to UTC if the stored offset is out of range.
fn local_time(ts: jiff::Timestamp, offset_seconds: i32) -> String {
    match jiff::tz::Offset::from_seconds(offset_seconds) {
        Ok(offset) => ts
            .to_zoned(jiff::tz::TimeZone::fixed(offset))
            .strftime("%Y-%m-%d %H:%M:%S %:z")
            .to_string(),
        Err(_) => ts.to_string(),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CaseConfig, Config, HashConfig, ReportConfig};
    use crate::hash::HashAlgorithm;
    use crate::journal::{CommandContext, Entry, ExitStatus, HostInfo, ProgramInfo};

    fn entry(seq: u64, kind: EntryKind, message: &str) -> Entry {
        Entry {
            log_version: 1,
            seq,
            prev: (seq > 0).then(|| format!("sha256:prev{seq}")),
            kind,
            examiner: "Jane".into(),
            case_id: "CASE-1".into(),
            message: message.into(),
            started_at: "2026-11-18T10:00:00Z".parse().unwrap(),
            finished_at: "2026-11-18T10:00:05Z".parse().unwrap(),
            host: HostInfo {
                hostname: "lab01".into(),
                user: "jane".into(),
                dflog_version: "0.1.0".into(),
                os: "macos".into(),
                arch: "aarch64".into(),
                utc_offset_seconds: 3600,
            },
            changes: vec![],
            external_changes: vec![],
        }
    }

    fn stat(size: u64, hash: &str) -> FileStat {
        FileStat {
            size,
            digest: Digest::Hash(hash.into()),
        }
    }

    fn render(report: &Report) -> String {
        let mut buf = Vec::new();
        TextRenderer.render(report, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    fn sample(verification: Verification) -> Report {
        let config = Config {
            case: CaseConfig {
                examiner: "Jane".into(),
                case_id: "CASE-1".into(),
            },
            hash: HashConfig {
                algorithm: HashAlgorithm::Sha256,
            },
            report: ReportConfig::default(),
        };
        let mut run = entry(
            1,
            EntryKind::Run {
                command: CommandContext {
                    argv: vec!["strings".into(), "disk.img".into()],
                    program: Some(ProgramInfo {
                        path: "/usr/bin/strings".into(),
                        digest: Digest::Hash("sha256:prog".into()),
                    }),
                    cwd: ".".into(),
                    status: ExitStatus::Exited(0),
                    stdout: CommandOutput {
                        path: "output/1.stdout".into(),
                        hash: "sha256:out".into(),
                        size: 42,
                    },
                    stderr: CommandOutput {
                        path: "output/1.stderr".into(),
                        hash: "sha256:err".into(),
                        size: 0,
                    },
                },
            },
            "extract strings",
        );
        run.changes = vec![
            FileChange {
                path: "strings.txt".into(),
                change: Change::Created {
                    after: stat(42, "sha256:new"),
                },
            },
            FileChange {
                path: "disk.img".into(),
                change: Change::Unchanged {
                    after: stat(1024, "sha256:img"),
                },
            },
        ];
        let mut record = entry(2, EntryKind::Record, "manual edit");
        record.external_changes = vec![FileChange {
            path: "notes.txt".into(),
            change: Change::Modified {
                before: stat(1, "sha256:a"),
                after: FileStat {
                    size: 2,
                    digest: Digest::Error("permission denied".into()),
                },
            },
        }];

        Report {
            case_id: "CASE-1".into(),
            examiner: "Jane".into(),
            generated_at: "2026-11-18T12:00:00Z".parse().unwrap(),
            dflog_version: "0.1.0",
            verification,
            journal_hash: "sha256:journal".into(),
            entries: vec![
                Ok(entry(0, EntryKind::Init { config }, "init")),
                Ok(run),
                Ok(record),
            ],
        }
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
}
