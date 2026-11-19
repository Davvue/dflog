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
use std::fmt::{Display, Formatter};
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

impl Entry {
    pub fn new(
        kind: EntryKind,
        config: &Config,
        message: String,
        started_at: jiff::Timestamp,
    ) -> Self {
        Entry {
            log_version: LOG_VERSION,
            seq: 0,
            prev: None,
            kind,
            examiner: config.case.examiner.clone(),
            case_id: config.case.case_id.clone(),
            message,
            started_at,
            finished_at: jiff::Timestamp::now(),
            host: HostInfo::current(),
            changes: vec![],
            external_changes: vec![],
        }
    }
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
    /// Replayed journal actions to create a new file index
    Reindex,
}

impl EntryKind {
    /// The name used for this kind in the serialized `kind` field.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Init { .. } => "init",
            Self::Run { .. } => "run",
            Self::Record => "record",
            Self::Reindex => "reindex",
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

impl Display for ExitStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ExitStatus::Exited(code) => {
                write!(f, "exit {}", code)
            }
            ExitStatus::Signal(sig) => {
                write!(f, "signal {}", sig)
            }
            ExitStatus::SpawnFailed(msg) => {
                write!(f, "spawn failed ({})", msg)
            }
        }
    }
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

impl HostInfo {
    pub fn current() -> Self {
        let now = jiff::Timestamp::now();

        HostInfo {
            hostname: gethostname::gethostname().to_string_lossy().into(),
            user: std::env::var("USER").unwrap_or("N/A".into()),
            dflog_version: env!("CARGO_PKG_VERSION").into(),
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            utc_offset_seconds: jiff::tz::TimeZone::system().to_offset(now).seconds(),
        }
    }
}

/// A change to one file. Serialized flat, e.g.
/// `{"path": "...", "op": "modified", "before": {...}, "after": {...}}`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// Path relative to the workspace root.
    #[serde(with = "crate::unix_path")]
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
    /// Recorded with its target instead of being followed
    Symlink(#[serde(with = "crate::unix_path")] std::path::PathBuf),
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

impl Display for Change {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Change::Created { .. } => {
                write!(f, "created")
            }
            Change::Modified { .. } => {
                write!(f, "modified")
            }
            Change::Deleted { .. } => {
                write!(f, "deleted")
            }
            Change::Unchanged { .. } => {
                write!(f, "unchanged")
            }
        }
    }
}

/// Everything recorded about one wrapped command.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct CommandContext {
    /// The command exactly as executed, one element per argument.
    pub argv: Vec<String>,
    /// The executable `argv[0]` resolved to.
    pub program: Option<ProgramInfo>,
    /// Working directory, relative to the workspace root.
    #[serde(with = "crate::unix_path")]
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
    #[serde(with = "crate::unix_path")]
    pub path: std::path::PathBuf,
    /// Algorithm-prefixed hash of the file's contents.
    pub hash: String,
    pub size: u64,
}

/// The executable that actually ran, for tool validation.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ProgramInfo {
    /// Absolute path after `PATH` lookup. On Windows this keeps its drive
    /// letter, e.g. `C:/Tools/x.exe`.
    #[serde(with = "crate::unix_path")]
    pub path: std::path::PathBuf,
    /// Serialized as `hash` or `error`, like in `FileStat`.
    #[serde(flatten)]
    pub digest: Digest,
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

pub struct JournalSnapshot {
    pub verification: Verification,
    pub file_hash: String,
    pub entries: Vec<Result<Entry>>,
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

        let (last_line, last) = last_entry(&contents)?;

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

    /// Checks that [`Journal::append`] would currently accept a new entry,
    /// without writing anything.
    ///
    /// Call this before doing anything that must be logged, e.g. before
    /// spawning the command in `dflog run`, so a damaged or unwritable
    /// journal stops the step before it happens instead of leaving it
    /// unrecorded. Opens the file for appending (so missing write permission
    /// is caught too) and fails like `append` would: [`Error::JournalCorrupt`]
    /// for an empty journal, an incomplete or unparseable last line.
    ///
    /// This is a pre-check, not a reservation: another process could still
    /// damage the journal between this call and `append`, which then still
    /// refuses to write.
    pub fn check_appendable(&self) -> Result<()> {
        let mut file = OpenOptions::new()
            .read(true)
            .append(true)
            .open(&self.path)?;
        file.lock_shared()?;

        let mut contents = Vec::new();
        file.read_to_end(&mut contents)?;
        last_entry(&contents)?;
        Ok(())
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

    pub fn entries(&self) -> Result<Vec<Result<Entry>>> {
        let mut file = OpenOptions::new().read(true).open(&self.path)?;
        file.lock_shared()?;

        let mut contents = Vec::new();
        file.read_to_end(&mut contents)?;
        drop(file);

        parse_entries(&contents)
    }

    pub fn snapshot(&self) -> Result<JournalSnapshot> {
        let mut file = OpenOptions::new().read(true).open(&self.path)?;
        file.lock_shared()?;
        let mut contents = Vec::new();
        file.read_to_end(&mut contents)?;
        drop(file);
        Ok(JournalSnapshot {
            verification: verify_bytes(&contents, self.algorithm)?,
            file_hash: self.algorithm.prefixed_digest(contents.as_slice())?,
            entries: parse_entries(&contents)?,
        })
    }
}

fn parse_entries(contents: &[u8]) -> Result<Vec<Result<Entry>>> {
    let body = contents.strip_suffix(b"\n").unwrap_or(contents);
    if body.is_empty() {
        return Ok(vec![]);
    }

    Ok(body
        .split(|&b| b == b'\n')
        .enumerate()
        .map(|(i, line)| {
            serde_json::from_slice::<Entry>(line)
                .map_err(|e| Error::JournalCorrupt(format!("line {}: {e}", i + 1)))
        })
        .collect())
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

/// The last line (as in [`last_line`]) and the entry it parses to. Shared by
/// [`Journal::append`] and [`Journal::check_appendable`] so both refuse
/// exactly the same journals.
fn last_entry(contents: &[u8]) -> Result<(&[u8], Entry)> {
    let line = last_line(contents)?;
    let entry = serde_json::from_slice(line)
        .map_err(|e| Error::JournalCorrupt(format!("last entry does not parse: {e}")))?;
    Ok((line, entry))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CaseConfig, HashConfig, ReportConfig};
    use std::path::PathBuf;

    // ---- fixtures -------------------------------------------------------

    fn config(algorithm: HashAlgorithm) -> Config {
        Config {
            case: CaseConfig {
                examiner: "Jane Doe".into(),
                case_id: "2026-0001".into(),
            },
            hash: HashConfig { algorithm },
            report: ReportConfig::default(),
        }
    }

    fn workspace(algorithm: HashAlgorithm) -> (tempfile::TempDir, Workspace) {
        let tmp = tempfile::tempdir().unwrap();
        let ws = Workspace::create(tmp.path(), config(algorithm)).unwrap();
        (tmp, ws)
    }

    fn entry(kind: EntryKind, message: &str) -> Entry {
        Entry {
            log_version: LOG_VERSION,
            seq: 0,
            prev: None,
            kind,
            examiner: "Jane Doe".into(),
            case_id: "2026-0001".into(),
            message: message.into(),
            started_at: jiff::Timestamp::from_second(1_700_000_000).unwrap(),
            finished_at: jiff::Timestamp::from_second(1_700_000_005).unwrap(),
            host: HostInfo {
                hostname: "forensic-ws".into(),
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

    fn init_entry(ws: &Workspace) -> Entry {
        entry(
            EntryKind::Init {
                config: ws.config.clone(),
            },
            "open case",
        )
    }

    fn stat(hash: &str, size: u64) -> FileStat {
        FileStat {
            size,
            digest: Digest::Hash(format!("sha256:{hash}")),
        }
    }

    /// A `run` entry that exercises every nested type, so serialization of
    /// all of them goes through the chain.
    fn run_entry(message: &str) -> Entry {
        let mut e = entry(
            EntryKind::Run {
                command: CommandContext {
                    argv: vec!["fls".into(), "-r".into(), "disk image.E01".into()],
                    program: Some(ProgramInfo {
                        path: "/usr/local/bin/fls".into(),
                        digest: Digest::Hash("sha256:00ff".into()),
                    }),
                    cwd: "working".into(),
                    status: ExitStatus::Exited(0),
                    stdout: CommandOutput {
                        path: "cmd-out/1700000000-123.stdout".into(),
                        hash: "sha256:aa".into(),
                        size: 42,
                    },
                    stderr: CommandOutput {
                        path: "cmd-out/1700000000-123.stderr".into(),
                        hash: "sha256:bb".into(),
                        size: 0,
                    },
                },
            },
            message,
        );
        e.changes = vec![
            FileChange {
                path: "working/out.txt".into(),
                change: Change::Created {
                    after: stat("01", 10),
                },
            },
            FileChange {
                path: "working/notes ü.md".into(),
                change: Change::Modified {
                    before: stat("02", 5),
                    after: stat("03", 6),
                },
            },
            FileChange {
                path: "working/tmp".into(),
                change: Change::Deleted {
                    before: stat("04", 1),
                },
            },
            FileChange {
                path: "working/disk image.E01".into(),
                change: Change::Unchanged {
                    after: FileStat {
                        size: 0,
                        digest: Digest::Error("permission denied".into()),
                    },
                },
            },
        ];
        e
    }

    fn record_entry(message: &str) -> Entry {
        entry(EntryKind::Record, message)
    }

    fn journal_path(ws: &Workspace) -> PathBuf {
        ws.dflog_dir().join(JOURNAL_FILE)
    }

    /// A workspace with a valid journal of four entries: init, run, record, run.
    fn populated(algorithm: HashAlgorithm) -> (tempfile::TempDir, Workspace, Journal) {
        let (tmp, ws) = workspace(algorithm);
        let journal = Journal::create(&ws, init_entry(&ws)).unwrap();
        journal.append(run_entry("list files")).unwrap();
        journal
            .append(record_entry("exported registry by hand"))
            .unwrap();
        journal.append(run_entry("list again")).unwrap();
        (tmp, ws, journal)
    }

    /// Bytes of a valid four-entry journal, for the `verify_bytes` tests.
    fn valid_bytes() -> Vec<u8> {
        let (_tmp, ws, _journal) = populated(HashAlgorithm::Sha256);
        std::fs::read(journal_path(&ws)).unwrap()
    }

    /// Splits journal bytes into lines without their `\n`.
    fn lines(bytes: &[u8]) -> Vec<Vec<u8>> {
        bytes
            .strip_suffix(b"\n")
            .unwrap()
            .split(|&b| b == b'\n')
            .map(<[u8]>::to_vec)
            .collect()
    }

    /// Joins lines back into journal bytes, each terminated by `\n`.
    fn join(lines: &[Vec<u8>]) -> Vec<u8> {
        lines
            .iter()
            .flat_map(|l| l.iter().copied().chain([b'\n']))
            .collect()
    }

    /// Parses a line, applies `f`, and serializes it again.
    fn edit(line: &[u8], f: impl FnOnce(&mut Entry)) -> Vec<u8> {
        let mut e: Entry = serde_json::from_slice(line).unwrap();
        f(&mut e);
        serde_json::to_vec(&e).unwrap()
    }

    /// Applies `f` to the lines of a valid journal and verifies the result.
    fn verify_tampered(f: impl FnOnce(&mut Vec<Vec<u8>>)) -> Verification {
        let mut l = lines(&valid_bytes());
        f(&mut l);
        verify_bytes(&join(&l), HashAlgorithm::Sha256).unwrap()
    }

    #[track_caller]
    fn assert_broken(v: Verification, line: u64, seq: Option<u64>, reason_contains: &str) {
        match v {
            Verification::Broken {
                line: l,
                seq: s,
                reason,
            } => {
                assert_eq!((l, s), (line, seq), "reason: {reason}");
                assert!(
                    reason.contains(reason_contains),
                    "expected reason containing {reason_contains:?}, got {reason:?}"
                );
            }
            v => panic!("expected Broken at line {line}, got {v:?}"),
        }
    }

    #[track_caller]
    fn assert_valid(v: &Verification, entries: u64) -> &str {
        match v {
            Verification::Valid { entries: n, head } => {
                assert_eq!(*n, entries);
                head
            }
            v => panic!("expected Valid, got {v:?}"),
        }
    }

    // ---- create / for_workspace / append --------------------------------

    #[test]
    fn create_append_verify_round_trip() {
        let (_tmp, ws, journal) = populated(HashAlgorithm::Sha256);

        let v = journal.verify().unwrap();
        let head = assert_valid(&v, 4);

        let bytes = std::fs::read(journal_path(&ws)).unwrap();
        let last = lines(&bytes).pop().unwrap();
        assert_eq!(
            head,
            HashAlgorithm::Sha256
                .prefixed_digest(last.as_slice())
                .unwrap()
        );
    }

    #[test]
    fn append_returns_exactly_what_is_on_disk() {
        let (_tmp, ws) = workspace(HashAlgorithm::Sha256);
        let journal = Journal::create(&ws, init_entry(&ws)).unwrap();
        let written = vec![
            journal.append(run_entry("one")).unwrap(),
            journal.append(record_entry("two")).unwrap(),
        ];

        let on_disk: Vec<Entry> = lines(&std::fs::read(journal_path(&ws)).unwrap())
            .iter()
            .skip(1)
            .map(|l| serde_json::from_slice(l).unwrap())
            .collect();
        assert_eq!(on_disk, written);
    }

    #[test]
    fn append_assigns_seq_and_prev_from_previous_line() {
        let (_tmp, ws) = workspace(HashAlgorithm::Sha256);
        let journal = Journal::create(&ws, init_entry(&ws)).unwrap();

        for expected_seq in 1..=3 {
            let before = lines(&std::fs::read(journal_path(&ws)).unwrap());
            let prev_line = before.last().unwrap();

            let e = journal.append(record_entry("step")).unwrap();
            assert_eq!(e.seq, expected_seq);
            assert_eq!(
                e.prev.as_deref(),
                Some(
                    HashAlgorithm::Sha256
                        .prefixed_digest(prev_line.as_slice())
                        .unwrap()
                        .as_str()
                )
            );
        }
    }

    #[test]
    fn create_overrides_caller_supplied_chain_fields() {
        let (_tmp, ws) = workspace(HashAlgorithm::Sha256);
        let mut init = init_entry(&ws);
        init.seq = 99;
        init.prev = Some("sha256:junk".into());
        init.log_version = LOG_VERSION + 7;
        Journal::create(&ws, init).unwrap();

        let line = lines(&std::fs::read(journal_path(&ws)).unwrap()).remove(0);
        let written: Entry = serde_json::from_slice(&line).unwrap();
        assert_eq!(
            (written.seq, written.prev, written.log_version),
            (0, None, LOG_VERSION)
        );
    }

    #[test]
    fn append_overrides_caller_supplied_chain_fields() {
        let (_tmp, ws) = workspace(HashAlgorithm::Sha256);
        let journal = Journal::create(&ws, init_entry(&ws)).unwrap();

        let mut e = record_entry("step");
        e.seq = 99;
        e.prev = Some("sha256:junk".into());
        e.log_version = LOG_VERSION + 7;
        let written = journal.append(e).unwrap();

        assert_eq!(written.seq, 1);
        assert_ne!(written.prev.as_deref(), Some("sha256:junk"));
        assert_eq!(written.log_version, LOG_VERSION);
        assert_valid(&journal.verify().unwrap(), 2);
    }

    #[test]
    fn create_rejects_non_init_entry_and_writes_nothing() {
        let (_tmp, ws) = workspace(HashAlgorithm::Sha256);
        assert!(matches!(
            Journal::create(&ws, record_entry("too early")),
            Err(Error::JournalEntryKind {
                expected: "init",
                got: "record"
            })
        ));
        assert!(matches!(
            Journal::create(&ws, run_entry("too early")),
            Err(Error::JournalEntryKind {
                expected: "init",
                got: "run"
            })
        ));
        assert!(!journal_path(&ws).exists());
    }

    #[test]
    fn create_never_overwrites_existing_journal() {
        let (_tmp, ws, _journal) = populated(HashAlgorithm::Sha256);
        let before = std::fs::read(journal_path(&ws)).unwrap();

        assert!(matches!(
            Journal::create(&ws, init_entry(&ws)),
            Err(Error::AlreadyInitialized(_))
        ));
        assert_eq!(std::fs::read(journal_path(&ws)).unwrap(), before);
    }

    #[test]
    fn for_workspace_opens_existing_journal() {
        let (_tmp, ws) = workspace(HashAlgorithm::Sha256);
        Journal::create(&ws, init_entry(&ws)).unwrap();

        let journal = Journal::for_workspace(&ws).unwrap();
        assert_eq!(journal.append(record_entry("later")).unwrap().seq, 1);
        assert_valid(&journal.verify().unwrap(), 2);
    }

    #[test]
    fn for_workspace_without_journal_fails() {
        let (_tmp, ws) = workspace(HashAlgorithm::Sha256);
        assert!(matches!(
            Journal::for_workspace(&ws),
            Err(Error::JournalNotFound(p)) if p == journal_path(&ws)
        ));
    }

    #[test]
    fn configured_algorithm_is_used_for_prev_and_head() {
        let (_tmp, _ws, journal) = populated(HashAlgorithm::Sha512);
        let e = journal.append(record_entry("step")).unwrap();
        assert!(e.prev.unwrap().starts_with("sha512:"));

        let v = journal.verify().unwrap();
        assert!(assert_valid(&v, 5).starts_with("sha512:"));
    }

    #[test]
    fn chain_with_mixed_algorithms_verifies() {
        // The algorithm in the config is changed mid-case: older entries keep
        // their sha256 prevs, newer ones use sha512, and the chain still holds.
        let (_tmp, mut ws, _journal) = populated(HashAlgorithm::Sha256);
        ws.config.hash.algorithm = HashAlgorithm::Sha512;
        let journal = Journal::for_workspace(&ws).unwrap();
        let e = journal.append(record_entry("after switch")).unwrap();
        assert!(e.prev.unwrap().starts_with("sha512:"));

        assert_valid(&journal.verify().unwrap(), 5);
    }

    fn assert_append_refused(contents: &[u8]) {
        let (_tmp, ws) = workspace(HashAlgorithm::Sha256);
        std::fs::write(journal_path(&ws), contents).unwrap();
        let journal = Journal::for_workspace(&ws).unwrap();

        // The pre-check must refuse exactly what append refuses.
        assert!(matches!(
            journal.check_appendable(),
            Err(Error::JournalCorrupt(_))
        ));
        assert!(matches!(
            journal.append(record_entry("x")),
            Err(Error::JournalCorrupt(_))
        ));
        assert_eq!(std::fs::read(journal_path(&ws)).unwrap(), contents);
    }

    #[test]
    fn check_appendable_accepts_valid_journal_and_writes_nothing() {
        let (_tmp, ws, journal) = populated(HashAlgorithm::Sha256);
        let before = std::fs::read(journal_path(&ws)).unwrap();

        journal.check_appendable().unwrap();

        assert_eq!(std::fs::read(journal_path(&ws)).unwrap(), before);
    }

    #[cfg(unix)]
    #[test]
    fn check_appendable_refuses_read_only_journal() {
        use std::os::unix::fs::PermissionsExt;

        let (_tmp, ws, journal) = populated(HashAlgorithm::Sha256);
        let path = journal_path(&ws);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();

        // Running as root ignores file permissions; nothing to check then.
        if std::fs::OpenOptions::new().append(true).open(&path).is_ok() {
            return;
        }
        assert!(matches!(journal.check_appendable(), Err(Error::Io(_))));
    }

    #[test]
    fn append_refuses_empty_journal() {
        assert_append_refused(b"");
    }

    #[test]
    fn append_refuses_incomplete_last_line() {
        let mut bytes = valid_bytes();
        bytes.truncate(bytes.len() - 20);
        assert_append_refused(&bytes);
    }

    #[test]
    fn append_refuses_unparseable_last_line() {
        let mut bytes = valid_bytes();
        bytes.extend_from_slice(b"{\"not\": \"an entry\"}\n");
        assert_append_refused(&bytes);
    }

    #[test]
    fn concurrent_appends_get_consecutive_seqs() {
        const THREADS: usize = 8;
        const PER_THREAD: usize = 10;

        let (_tmp, ws) = workspace(HashAlgorithm::Sha256);
        Journal::create(&ws, init_entry(&ws)).unwrap();

        std::thread::scope(|s| {
            for _ in 0..THREADS {
                s.spawn(|| {
                    // Each thread opens its own handle, as separate processes would.
                    let journal = Journal::for_workspace(&ws).unwrap();
                    for _ in 0..PER_THREAD {
                        journal.append(record_entry("parallel")).unwrap();
                    }
                });
            }
        });

        let journal = Journal::for_workspace(&ws).unwrap();
        assert_valid(
            &journal.verify().unwrap(),
            1 + (THREADS * PER_THREAD) as u64,
        );
    }

    // ---- verify_bytes: tampering and damage -----------------------------

    #[test]
    fn valid_journal_verifies() {
        let v = verify_bytes(&valid_bytes(), HashAlgorithm::Sha256).unwrap();
        assert_valid(&v, 4);
    }

    #[test]
    fn head_uses_requested_algorithm() {
        let v = verify_bytes(&valid_bytes(), HashAlgorithm::Sha512).unwrap();
        assert!(assert_valid(&v, 4).starts_with("sha512:"));
    }

    #[test]
    fn empty_journal_is_broken() {
        let v = verify_bytes(b"", HashAlgorithm::Sha256).unwrap();
        assert_broken(v, 1, None, "empty");
    }

    #[test]
    fn missing_trailing_newline_is_broken() {
        let mut bytes = valid_bytes();
        bytes.pop();
        let v = verify_bytes(&bytes, HashAlgorithm::Sha256).unwrap();
        assert_broken(v, 4, None, "no trailing newline");
    }

    #[test]
    fn cut_off_last_line_is_broken() {
        let mut bytes = valid_bytes();
        bytes.truncate(bytes.len() - 30);
        let v = verify_bytes(&bytes, HashAlgorithm::Sha256).unwrap();
        assert_broken(v, 4, None, "no trailing newline");
    }

    #[test]
    fn edited_middle_entry_breaks_next_line() {
        let v = verify_tampered(|l| l[1] = edit(&l[1], |e| e.message = "nothing to see".into()));
        assert_broken(v, 3, Some(2), "prev does not match hash of line 2");
    }

    #[test]
    fn single_byte_change_breaks_chain() {
        // A change that keeps the JSON valid and the entry identical after
        // parsing (extra whitespace) must still be detected: the chain is over
        // raw bytes, not over parsed entries.
        let v = verify_tampered(|l| l[1].insert(1, b' '));
        assert_broken(v, 3, Some(2), "prev does not match");
    }

    #[test]
    fn edited_first_entry_breaks_second_line() {
        let v = verify_tampered(|l| l[0] = edit(&l[0], |e| e.examiner = "Mallory".into()));
        assert_broken(v, 2, Some(1), "prev does not match hash of line 1");
    }

    #[test]
    fn edited_last_entry_changes_head() {
        // The chain can't detect this on its own; the head recorded elsewhere
        // can.
        let original = verify_bytes(&valid_bytes(), HashAlgorithm::Sha256).unwrap();
        let tampered = verify_tampered(|l| l[3] = edit(&l[3], |e| e.message = "edited".into()));
        assert_ne!(assert_valid(&original, 4), assert_valid(&tampered, 4));
    }

    #[test]
    fn deleted_middle_entry_is_detected() {
        let v = verify_tampered(|l| {
            l.remove(1);
        });
        assert_broken(v, 2, Some(2), "expected seq 1, found 2");
    }

    #[test]
    fn swapped_entries_are_detected() {
        let v = verify_tampered(|l| l.swap(1, 2));
        assert_broken(v, 2, Some(2), "expected seq 1, found 2");
    }

    #[test]
    fn deleted_middle_entry_with_renumbered_seqs_is_detected() {
        // An attacker who also fixes up the seq numbers still breaks the hash.
        let v = verify_tampered(|l| {
            l.remove(1);
            l[1] = edit(&l[1], |e| e.seq = 1);
            l[2] = edit(&l[2], |e| e.seq = 2);
        });
        assert_broken(v, 2, Some(1), "prev does not match hash of line 1");
    }

    #[test]
    fn truncated_trailing_entries_change_head() {
        let original = verify_bytes(&valid_bytes(), HashAlgorithm::Sha256).unwrap();
        let truncated = verify_tampered(|l| l.truncate(2));
        assert_ne!(assert_valid(&original, 4), assert_valid(&truncated, 2));
    }

    #[test]
    fn garbage_line_is_broken() {
        let v = verify_tampered(|l| l.push(b"not json".to_vec()));
        assert_broken(v, 5, None, "does not parse");
    }

    #[test]
    fn blank_line_is_broken() {
        let v = verify_tampered(|l| l.insert(2, vec![]));
        assert_broken(v, 3, None, "does not parse");
    }

    #[test]
    fn crlf_line_ending_is_detected() {
        // serde_json accepts the trailing `\r` as whitespace, so line 2 still
        // parses. Its hash changes, so line 3 must fail.
        let v = verify_tampered(|l| l[1].push(b'\r'));
        assert_broken(v, 3, Some(2), "prev does not match");
    }

    #[test]
    fn first_entry_must_be_init() {
        let v = verify_tampered(|l| {
            l.remove(0);
        });
        assert_broken(v, 1, Some(1), "first entry must be init, found run");
    }

    #[test]
    fn first_entry_must_have_seq_zero() {
        let v = verify_tampered(|l| l[0] = edit(&l[0], |e| e.seq = 5));
        assert_broken(v, 1, Some(5), "first entry must have seq 0");
    }

    #[test]
    fn first_entry_must_not_have_prev() {
        let v = verify_tampered(|l| l[0] = edit(&l[0], |e| e.prev = Some("sha256:00".into())));
        assert_broken(v, 1, Some(0), "must not have a prev");
    }

    #[test]
    fn second_init_entry_is_broken() {
        let v = verify_tampered(|l| {
            let init = edit(&l[0], |e| e.seq = 4);
            l.push(init);
        });
        assert_broken(v, 5, Some(4), "init entry after first line");
    }

    #[test]
    fn seq_gap_is_broken() {
        // Line 3's prev still matches line 2, so only the seq check fires.
        let v = verify_tampered(|l| l[2] = edit(&l[2], |e| e.seq = 7));
        assert_broken(v, 3, Some(7), "expected seq 2, found 7");
    }

    #[test]
    fn missing_prev_is_broken() {
        let v = verify_tampered(|l| l[2] = edit(&l[2], |e| e.prev = None));
        assert_broken(v, 3, Some(2), "missing prev");
    }

    #[test]
    fn prev_without_prefix_is_broken() {
        let v = verify_tampered(|l| {
            l[2] = edit(&l[2], |e| {
                let p = e.prev.take().unwrap();
                e.prev = Some(p.split_once(':').unwrap().1.to_string());
            })
        });
        assert_broken(v, 3, Some(2), "no algorithm prefix");
    }

    #[test]
    fn prev_with_unknown_algorithm_is_broken() {
        let v = verify_tampered(|l| {
            l[2] = edit(&l[2], |e| {
                let p = e.prev.take().unwrap();
                e.prev = Some(format!("md5:{}", p.split_once(':').unwrap().1));
            })
        });
        assert_broken(v, 3, Some(2), "unsupported hash algorithm in prev: md5");
    }

    #[test]
    fn prev_with_uppercase_hex_is_broken() {
        // Hashes are compared exactly; there is one canonical form.
        let v = verify_tampered(|l| {
            l[2] = edit(&l[2], |e| {
                let p = e.prev.take().unwrap();
                let (alg, hex) = p.split_once(':').unwrap();
                e.prev = Some(format!("{alg}:{}", hex.to_uppercase()));
            })
        });
        assert_broken(v, 3, Some(2), "prev does not match");
    }

    #[test]
    fn unsupported_log_version_is_broken() {
        let v = verify_tampered(|l| l[1] = edit(&l[1], |e| e.log_version = LOG_VERSION + 1));
        assert_broken(v, 2, Some(1), "unsupported log version");
    }
}
