use std::fs::OpenOptions;
use std::io::{Read, Write};
use serde::{Deserialize, Serialize};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::hash::HashAlgorithm;
use crate::workspace::Workspace;

pub const LOG_VERSION: u32 = 1;
const JOURNAL_FILE: &str = "journal.jsonl";

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub log_version: u32,
    pub seq: u64,
    pub prev: Option<String>,
    #[serde(flatten)]
    pub kind: EntryKind,
    pub examiner: String,
    pub case_id: String,
    pub message: String,
    pub started_at: jiff::Timestamp,
    pub finished_at: jiff::Timestamp,
    pub host: HostInfo,
    pub changes: Vec<FileChange>,
    pub external_changes: Vec<FileChange>
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EntryKind {
    Init { config: Config },
    Run { command: CommandContext },
    Record,
}

impl EntryKind {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Init { .. } => "init",
            Self::Run { .. } => "run",
            Self::Record => "record",
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExitStatus { Exited(i32), Signal(i32), SpawnFailed(String) }

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct HostInfo {
    pub hostname: String,
    pub user: String,
    pub dflog_version: String,
    pub os: String,
    pub arch: String,
    pub utc_offset_seconds: i32
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: std::path::PathBuf,
    #[serde(flatten)]
    pub change: Change,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct FileStat {
    pub size: u64,
    #[serde(flatten)]
    pub digest: Digest
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Digest { Hash(String), Error(String) }

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Change {
    Created { after: FileStat },
    Modified { before: FileStat, after: FileStat },
    Deleted { before: FileStat },
    Unchanged { after: FileStat },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct CommandContext {
    pub argv: Vec<String>,
    pub program: ProgramInfo,
    pub cwd: std::path::PathBuf,
    pub status: ExitStatus,
    pub stdout: CommandOutput,
    pub stderr: CommandOutput,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub path: std::path::PathBuf,
    pub hash: String,
    pub size: u64
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ProgramInfo {
    pub path: std::path::PathBuf,
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verification {
    Valid { entries: u64, head: String },
    Broken { line: u64, seq: Option<u64>, reason: String },
}

pub struct Journal {
    path: std::path::PathBuf,
    algorithm: HashAlgorithm
}

impl Journal {
    pub fn create(ws: &Workspace, mut init_entry: Entry) -> Result<Self> {
        if !matches!(init_entry.kind, EntryKind::Init { .. }) {
            return Err(Error::JournalEntryKind { expected: "init", got: init_entry.kind.name() });
        }

        init_entry.seq = 0;
        init_entry.prev = None;
        init_entry.log_version = LOG_VERSION;
        let ws_log = ws.dflog_dir().join(JOURNAL_FILE);
        let serialized = serde_json::to_string(&init_entry)?;

        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&ws_log)
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::AlreadyExists => Error::AlreadyInitialized(ws.root.clone()),
                _ => Error::Io(e)
            })?;

        file.write_all(serialized.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;

        Ok(Journal {
            path: ws_log,
            algorithm: ws.config.hash.algorithm,
        })
    }

    pub fn for_workspace(ws: &Workspace) -> Result<Self> {
        let ws_log = ws.dflog_dir().join(JOURNAL_FILE);

        if !ws_log.try_exists()? {
            return Err(Error::JournalNotFound(ws_log));
        }

        Ok(Journal {
            path: ws_log,
            algorithm: ws.config.hash.algorithm
        })
    }

    pub fn append(&self, mut entry: Entry) -> Result<Entry> {
        let mut file = OpenOptions::new().read(true).append(true).open(&self.path)?;
        file.lock()?;

        let mut contents = Vec::new();
        file.read_to_end(&mut contents)?;

        let last_line = last_line(&contents)?;
        let last: Entry = serde_json::from_slice(last_line)
            .map_err(|e| Error::JournalCorrupt(format!("last entry does not parse: {e}")))?;

        entry.seq = last.seq.checked_add(1).ok_or_else(|| Error::JournalCorrupt("seq overflow".into()))?;
        entry.prev = Some(self.algorithm.prefixed_digest(last_line)?);
        entry.log_version = LOG_VERSION;

        let mut line = serde_json::to_vec(&entry)?;
        line.push(b'\n');
        file.write_all(&line)?;
        file.sync_all()?;

        Ok(entry)
    }

    pub fn verify(&self) -> Result<Verification> {
        let mut file = OpenOptions::new().read(true).open(&self.path)?;
        file.lock_shared()?;

        let mut contents = Vec::new();
        file.read_to_end(&mut contents)?;
        verify_bytes(&contents, self.algorithm)
    }
}

fn last_line(contents: &[u8]) -> Result<&[u8]> {
    let body = contents.strip_suffix(b"\n").ok_or_else(|| Error::JournalCorrupt(
        if contents.is_empty() { "journal is empty (missing init entry)".into() }
        else { "last entry is incomplete (no trailing newline)".into() }
    ))?;
    let start = body.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    Ok(&body[start..])
}

fn verify_bytes(contents: &[u8], head_algorithm: HashAlgorithm) -> Result<Verification> {
    let broken = |line: u64, seq: Option<u64>, reason: String| {
        Ok(Verification::Broken { line, seq, reason })
    };

    let Some(body) = contents.strip_suffix(b"\n") else {
        return if contents.is_empty() {
            broken(1, None, "journal is empty (missing init entry)".into())
        } else {
            let line = contents.iter().filter(|&&b| b == b'\n').count() as u64 + 1;
            broken(line, None, "last entry is incomplete (no trailing newline)".into())
        }
    };

    let mut prev: Option<(&[u8], u64)> = None;
    let mut line_no = 0;

    for line in body.split(|&b| b == b'\n') {
        line_no += 1;

        let entry: Entry = match serde_json::from_slice(line) {
            Ok(entry) => entry,
            Err(e) => return broken(line_no, None, format!("entry does not parse: {e}"))
        };
        let seq = Some(entry.seq);

        if entry.log_version != LOG_VERSION {
            return broken(line_no, seq, format!("unsupported log version: {}", entry.log_version));
        };

        match prev {
            None => {
                if !matches!(entry.kind, EntryKind::Init { .. }) {
                    return broken(line_no, seq, format!("first entry must be init, found {}", entry.kind.name()));
                }
                if entry.seq != 0 {
                    return broken(line_no, seq, format!("first entry must have seq 0, found {}", entry.seq));
                }
                if entry.prev.is_some() {
                    return broken(line_no, seq, "first entry must not have a prev".into());
                }
            },
            Some((prev_line, prev_seq)) => {
                if matches!(entry.kind, EntryKind::Init { .. }) {
                    return broken(line_no, seq, "init entry after first line".into());
                }
                if Some(entry.seq) != prev_seq.checked_add(1) {
                    return broken(line_no, seq, format!("expected seq {}, found {}", prev_seq.wrapping_add(1), entry.seq));
                }
                let Some(prev_hash) = &entry.prev else {
                    return broken(line_no, seq, "missing prev".into());
                };
                let Some((name, _)) = prev_hash.split_once(':') else {
                    return broken(line_no, seq, format!("prev has no algorithm prefix: {prev_hash}"));
                };
                let Ok(algorithm) = name.parse::<HashAlgorithm>() else {
                    return broken(line_no, seq, format!("unsupported hash algorithm in prev: {name}"));
                };
                if *prev_hash != algorithm.prefixed_digest(prev_line)? {
                    return broken(line_no, seq, format!("prev does not match hash of line {}", line_no - 1));
                }
            }
        }

        prev = Some((line, entry.seq))
    };

    let (last_line, _) = prev.expect("loop ran at least once");
    Ok(Verification::Valid {
        entries: line_no,
        head: head_algorithm.prefixed_digest(last_line)?,
    })
}