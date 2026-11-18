//! A sample report shared by the renderer tests.

use crate::config::{CaseConfig, Config, HashConfig, ReportConfig};
use crate::hash::HashAlgorithm;
use crate::journal::{
    Change, CommandContext, CommandOutput, Digest, Entry, EntryKind, ExitStatus, FileChange,
    FileStat, HostInfo, ProgramInfo, Verification,
};
use crate::report_renderer::Report;

pub fn entry(seq: u64, kind: EntryKind, message: &str) -> Entry {
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

/// Three entries (init, run, record) covering every change kind and a
/// digest that could not be computed.
pub fn sample(verification: Verification) -> Report {
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
