//! The case journal saved as an append-only, hash-chained JSON Lines file
//!
//! Each line is one [`Entry`]. Entry 0 must always be an `init` entry with no
//! `prev`. Every later entry's `prev` is the algorithm-prefixed hash of the
//! previous line's raw bytes, without its trailing `\n`. Editing, removing or
//! reordering an entry therefore breaks the chain at the following line, which
//! [`Journal::verify`] detects. Removing entries from the end does not break
//! the chain. That is caught by comparing the `head` hash against a value
//! recorded elsewhere, for example in the case notes.

use crate::config::Config;
use crate::error::{Error, Result};
use crate::hash::HashAlgorithm;
use crate::workspace::Workspace;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::{Read, Write};

/// Version of the entry format written by this build. [`Journal::create`] and
/// [`Journal::append`] always set it, and [`Journal::verify`] rejects any
/// other value.
pub const LOG_VERSION: u32 = 1;

/// File name of the journal inside the workspace directory.
const JOURNAL_FILE: &str = "journal.jsonl";

/// One line of the journal.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Format version of this entry, see [`LOG_VERSION`].
    pub log_version: u32,
    /// Position in the chain, starting at 0 with no gaps. Assigned by the
    /// journal; values set by the caller are overwritten.
    pub seq: u64,
    /// Algorithm-prefixed hash of the previous line (e.g. `"sha256:..."`),
    /// `None` only for entry 0. Assigned by the journal.
    pub prev: Option<String>,
    /// What kind of entry this is. Serialized as a `kind` field on the entry
    /// itself, together with the kind's own fields.
    #[serde(flatten)]
    pub kind: EntryKind,
    /// Examiner responsible at the time of the entry. Stored per entry so that
    /// each entry stands on its own even if the config changes later.
    pub examiner: String,
    /// Case identifier, stored per entry for the same reason as `examiner`.
    pub case_id: String,
    /// The examiner's reason for this step.
    pub message: String,
    pub started_at: jiff::Timestamp,
    pub finished_at: jiff::Timestamp,
    pub host: HostInfo,
    /// Files created, modified or deleted during this entry.
    pub changes: Vec<FileChange>,
    /// Changes found before this entry started that were made outside dflog
    /// since the previous entry.
    pub external_changes: Vec<FileChange>,
}

/// The kind of a journal entry, with its kind-specific data.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EntryKind {
    /// First entry of every journal. Records the configuration the case was
    /// opened with.
    Init { config: Config },
    /// A command executed through `dflog run`.
    Run { command: CommandContext },
    /// Changes the examiner made outside dflog and documented afterwards.
    Record,
}

impl EntryKind {
    /// The name used for this kind in the serialized `kind` field.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Init { .. } => "init",
            Self::Run { .. } => "run",
            Self::Record => "record",
        }
    }
}

/// How a wrapped command ended.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExitStatus {
    /// The process exited with this code.
    Exited(i32),
    /// The process was terminated by this signal (Unix only).
    Signal(i32),
    /// The process could not be started. Contains the OS error message.
    SpawnFailed(String),
}

/// The machine and dflog build an entry was written on.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct HostInfo {
    pub hostname: String,
    pub user: String,
    pub dflog_version: String,
    /// `std::env::consts::OS`, e.g. `"macos"`.
    pub os: String,
    /// `std::env::consts::ARCH`, e.g. `"aarch64"`.
    pub arch: String,
    /// Local UTC offset when the entry was written. Timestamps are stored in
    /// UTC; this only allows showing local time in reports.
    pub utc_offset_seconds: i32,
}

/// A change to one file. Serialized flat, e.g.
/// `{"path": "...", "op": "modified", "before": {...}, "after": {...}}`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// Path relative to the workspace root.
    pub path: std::path::PathBuf,
    #[serde(flatten)]
    pub change: Change,
}

/// Size and hash of a file at one point in time.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct FileStat {
    pub size: u64,
    #[serde(flatten)]
    pub digest: Digest,
}

/// Result of hashing a file. Serialized as either a `hash` or an `error` field.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Digest {
    /// Algorithm-prefixed hash, e.g. `"sha256:..."`.
    Hash(String),
    /// The file could not be read (e.g. permission denied). It is recorded
    /// with the error instead of being left out of the entry.
    Error(String),
}

/// What happened to a file. Each variant carries exactly the states that
/// exist for it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Change {
    Created {
        after: FileStat,
    },
    Modified {
        before: FileStat,
        after: FileStat,
    },
    Deleted {
        before: FileStat,
    },
    /// A file named as an artifact that the step did not change, e.g. an
    /// input. Recording it shows that the source was left intact.
    Unchanged {
        after: FileStat,
    },
}

/// Everything recorded about one wrapped command.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct CommandContext {
    /// The command exactly as executed, one element per argument.
    pub argv: Vec<String>,
    /// The executable `argv[0]` resolved to.
    pub program: ProgramInfo,
    /// Working directory, relative to the workspace root.
    pub cwd: std::path::PathBuf,
    pub status: ExitStatus,
    pub stdout: CommandOutput,
    pub stderr: CommandOutput,
}

/// A captured output stream. Files are named by run start
/// time and process ID, not by `seq`, because `seq` is only
/// assigned when the entry is appended.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    /// Path relative to the `.dflog` directory.
    pub path: std::path::PathBuf,
    /// Algorithm-prefixed hash of the file's contents.
    pub hash: String,
    pub size: u64,
}

/// The executable that actually ran, for tool validation.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ProgramInfo {
    /// Absolute path after `PATH` lookup.
    pub path: std::path::PathBuf,
    /// Algorithm-prefixed hash of the executable.
    pub hash: String,
}

/// Outcome of [`Journal::verify`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verification {
    /// The whole chain is intact. `head` is the prefixed hash of the last
    /// line. Record it in the case notes so that later removal of trailing
    /// entries can be detected.
    Valid { entries: u64, head: String },
    /// The first problem found. Nothing from `line` onwards can be trusted.
    /// `line` is 1-based, as an editor shows it. `seq` is `None` if the line
    /// could not be parsed.
    Broken {
        line: u64,
        seq: Option<u64>,
        reason: String,
    },
}

/// Handle to a workspace's journal file. Holds no open file; each operation
/// opens, locks and closes the file itself.
pub struct Journal {
    path: std::path::PathBuf,
    /// Algorithm used for new `prev` hashes and for `head`. Verification uses
    /// the algorithm named in each `prev` instead.
    algorithm: HashAlgorithm,
}

impl Journal {
    /// Creates the journal and writes `init_entry` as entry 0.
    ///
    /// `seq`, `prev` and `log_version` are set here. Fails with
    /// [`Error::JournalEntryKind`] if the entry is not an `init` entry, and
    /// with [`Error::AlreadyInitialized`] if a journal already exists. An
    /// existing journal is never overwritten.
    pub fn create(ws: &Workspace, mut init_entry: Entry) -> Result<Self> {
        if !matches!(init_entry.kind, EntryKind::Init { .. }) {
            return Err(Error::JournalEntryKind {
                expected: "init",
                got: init_entry.kind.name(),
            });
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
                _ => Error::Io(e),
            })?;

        file.write_all(serialized.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;

        Ok(Journal {
            path: ws_log,
            algorithm: ws.config.hash.algorithm,
        })
    }

    /// Opens the journal of an existing workspace.
    ///
    /// Fails with [`Error::JournalNotFound`] if the file is missing. The
    /// contents are not checked here; use [`Journal::verify`] for that.
    pub fn for_workspace(ws: &Workspace) -> Result<Self> {
        let ws_log = ws.dflog_dir().join(JOURNAL_FILE);

        if !ws_log.try_exists()? {
            return Err(Error::JournalNotFound(ws_log));
        }

        Ok(Journal {
            path: ws_log,
            algorithm: ws.config.hash.algorithm,
        })
    }

    /// Appends `entry` to the journal and returns it as written.
    ///
    /// `seq`, `prev` and `log_version` are set here; whatever the caller put in
    /// them is overwritten. Holds an exclusive lock on the file for the whole
    /// operation, so concurrent appends from several processes get
    /// consecutive `seq` values. The entry and its newline are written in a
    /// single write, followed by `sync_all`.
    ///
    /// Fails with [`Error::JournalCorrupt`] instead of appending if the journal
    /// is empty, its last line is incomplete (no trailing newline) or does not
    /// parse, so a damaged journal is never silently extended.
    pub fn append(&self, mut entry: Entry) -> Result<Entry> {
        let mut file = OpenOptions::new()
            .read(true)
            .append(true)
            .open(&self.path)?;
        file.lock()?;

        let mut contents = Vec::new();
        file.read_to_end(&mut contents)?;

        let last_line = last_line(&contents)?;
        let last: Entry = serde_json::from_slice(last_line)
            .map_err(|e| Error::JournalCorrupt(format!("last entry does not parse: {e}")))?;

        entry.seq = last
            .seq
            .checked_add(1)
            .ok_or_else(|| Error::JournalCorrupt("seq overflow".into()))?;
        entry.prev = Some(self.algorithm.prefixed_digest(last_line)?);
        entry.log_version = LOG_VERSION;

        let mut line = serde_json::to_vec(&entry)?;
        line.push(b'\n');
        file.write_all(&line)?;
        file.sync_all()?;

        Ok(entry)
    }

    /// Checks the whole hash chain from entry 0 to the last line.
    ///
    /// `Err` means only that the journal could not be read. A damaged or
    /// tampered journal is a finding, not an error, and is returned as
    /// [`Verification::Broken`]. Takes a shared lock, so it waits for an
    /// append in progress but can run alongside other verifies.
    pub fn verify(&self) -> Result<Verification> {
        let mut file = OpenOptions::new().read(true).open(&self.path)?;
        file.lock_shared()?;

        let mut contents = Vec::new();
        file.read_to_end(&mut contents)?;
        verify_bytes(&contents, self.algorithm)
    }
}

/// Returns the last line of the journal, without its trailing `\n`.
///
/// These exact bytes are what the next entry's `prev` hashes, and
/// [`verify_bytes`] must hash the same bytes. Fails if the journal is empty or
/// does not end with `\n`, which means the last write was cut off.
fn last_line(contents: &[u8]) -> Result<&[u8]> {
    let body = contents.strip_suffix(b"\n").ok_or_else(|| {
        Error::JournalCorrupt(if contents.is_empty() {
            "journal is empty (missing init entry)".into()
        } else {
            "last entry is incomplete (no trailing newline)".into()
        })
    })?;
    let start = body.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    Ok(&body[start..])
}

/// The chain check behind [`Journal::verify`], kept separate from file I/O so
/// tests can pass in bytes directly.
///
/// Lines are split on raw `\n` bytes and hashed without it, exactly like
/// [`last_line`]. Never use `str::lines` here: it also strips a trailing `\r`,
/// so a line saved with `\r\n` would hash differently from what is on disk.
/// Each `prev` is checked with the algorithm named in its own prefix, so the
/// chain can be verified without the config. `head_algorithm` is used only for
/// the returned `head`.
///
/// Checks, stopping at the first failure: every line parses as an [`Entry`]
/// with a supported `log_version`. Line 1 is `init` with `seq` 0 and no
/// `prev`. Every later line is not `init`, has `seq` equal to the previous
/// `seq` + 1, and has a `prev` with a known algorithm prefix that matches the
/// previous line's hash.
fn verify_bytes(contents: &[u8], head_algorithm: HashAlgorithm) -> Result<Verification> {
    let broken = |line: u64, seq: Option<u64>, reason: String| {
        Ok(Verification::Broken { line, seq, reason })
    };

    let Some(body) = contents.strip_suffix(b"\n") else {
        return if contents.is_empty() {
            broken(1, None, "journal is empty (missing init entry)".into())
        } else {
            let line = contents.iter().filter(|&&b| b == b'\n').count() as u64 + 1;
            broken(
                line,
                None,
                "last entry is incomplete (no trailing newline)".into(),
            )
        };
    };

    let mut prev: Option<(&[u8], u64)> = None;
    let mut line_no = 0;

    for line in body.split(|&b| b == b'\n') {
        line_no += 1;

        let entry: Entry = match serde_json::from_slice(line) {
            Ok(entry) => entry,
            Err(e) => return broken(line_no, None, format!("entry does not parse: {e}")),
        };
        let seq = Some(entry.seq);

        if entry.log_version != LOG_VERSION {
            return broken(
                line_no,
                seq,
                format!("unsupported log version: {}", entry.log_version),
            );
        };

        match prev {
            None => {
                if !matches!(entry.kind, EntryKind::Init { .. }) {
                    return broken(
                        line_no,
                        seq,
                        format!("first entry must be init, found {}", entry.kind.name()),
                    );
                }
                if entry.seq != 0 {
                    return broken(
                        line_no,
                        seq,
                        format!("first entry must have seq 0, found {}", entry.seq),
                    );
                }
                if entry.prev.is_some() {
                    return broken(line_no, seq, "first entry must not have a prev".into());
                }
            }
            Some((prev_line, prev_seq)) => {
                if matches!(entry.kind, EntryKind::Init { .. }) {
                    return broken(line_no, seq, "init entry after first line".into());
                }
                if Some(entry.seq) != prev_seq.checked_add(1) {
                    return broken(
                        line_no,
                        seq,
                        format!(
                            "expected seq {}, found {}",
                            prev_seq.wrapping_add(1),
                            entry.seq
                        ),
                    );
                }
                let Some(prev_hash) = &entry.prev else {
                    return broken(line_no, seq, "missing prev".into());
                };
                let Some((name, _)) = prev_hash.split_once(':') else {
                    return broken(
                        line_no,
                        seq,
                        format!("prev has no algorithm prefix: {prev_hash}"),
                    );
                };
                let Ok(algorithm) = name.parse::<HashAlgorithm>() else {
                    return broken(
                        line_no,
                        seq,
                        format!("unsupported hash algorithm in prev: {name}"),
                    );
                };
                if *prev_hash != algorithm.prefixed_digest(prev_line)? {
                    return broken(
                        line_no,
                        seq,
                        format!("prev does not match hash of line {}", line_no - 1),
                    );
                }
            }
        }

        prev = Some((line, entry.seq))
    }

    let (last_line, _) = prev.expect("loop ran at least once");
    Ok(Verification::Valid {
        entries: line_no,
        head: head_algorithm.prefixed_digest(last_line)?,
    })
}
