use serde::{Deserialize, Serialize};
use crate::config::Config;

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