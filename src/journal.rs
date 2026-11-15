use std::fs::OpenOptions;
use std::io::{Read, Write};
use serde::{Deserialize, Serialize};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::hash::HashAlgorithm;
use crate::workspace::Workspace;

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
        let ws_log = ws.dflog_dir().join(JOURNAL_FILE);
        let serialized = serde_json::to_string(&init_entry)?;

        let mut file = std::fs::OpenOptions::new()
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
        
        let mut line = serde_json::to_vec(&entry)?;
        line.push(b'\n');
        file.write_all(&line)?;
        file.sync_all()?;

        Ok(entry)
    }

    pub fn verify(&self) -> Result<()> {
        Ok(())
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