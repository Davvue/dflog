//! The crate-wide error type. The `#[error]` messages are what the examiner
//! sees: `main` prints them as `dflog error: <message>`.

use std::path::PathBuf;

/// Everything that can make a dflog command fail.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No `.dflog` directory was found in the start directory or any parent.
    /// Contains the start directory.
    #[error("not a dflog workspace (or any parent up to /): {0}")]
    WorkspaceNotFound(PathBuf),

    /// `init` was run where a workspace (its config or journal) already
    /// exists. Nothing is overwritten.
    #[error("workspace already initialized at {0}")]
    AlreadyInitialized(PathBuf),

    /// The config file exists but could not be read.
    #[error("could not read config {path}: {source}")]
    ConfigRead {
        path: PathBuf,
        source: std::io::Error,
    },

    /// The config file is not valid TOML, has unknown keys or is missing
    /// required keys.
    #[error("invalid config {path}: {source}")]
    ConfigParse {
        path: PathBuf,
        source: toml::de::Error,
    },

    /// The config could not be turned into TOML. Indicates a bug, not a
    /// user error.
    #[error("could not serialize config: {0}")]
    ConfigSerialize(#[from] toml::ser::Error),

    /// A journal operation got an entry of the wrong kind, e.g. a non-`init`
    /// entry passed to `Journal::create`.
    #[error("unexpected journal entry kind: expected {expected}, found {got}")]
    JournalEntryKind {
        expected: &'static str,
        got: &'static str,
    },

    /// The workspace exists but has no journal file.
    #[error("journal file missing: {0}")]
    JournalNotFound(PathBuf),

    /// An entry could not be turned into JSON. Indicates a bug, not a user
    /// error. Parsing errors are reported as [`Error::JournalCorrupt`]
    /// instead.
    #[error("could not serialize to JSON: {0}")]
    JsonSerialize(#[from] serde_json::Error),

    /// The journal is in a state that must not be appended to: empty, last
    /// line incomplete, or last entry unparseable. Raised by `append`;
    /// `verify` reports such problems as a result instead of an error.
    #[error("journal is corrupt: {0}")]
    JournalCorrupt(String),

    #[error("index ({path}) is corrupt: {source}")]
    IndexCorrupt {
        path: PathBuf,
        source: serde_json::Error,
    },

    /// The index file at the specified path does not exist
    #[error("file index missing: {0}")]
    IndexNotFound(PathBuf),

    #[error("found non utf-8 path: {0}")]
    NonUtf8Path(PathBuf),

    /// A hash algorithm name that dflog does not support.
    #[error("unsupported hash algorithm: {0}")]
    UnsupportedHashAlgorithm(String),

    /// An export format that dflog does not support
    #[error("unsupported export format: {0}")]
    UnsupportedExportFormat(String),

    /// The specified default name format for reports is invalid
    #[error("invalid report name format: {0}")]
    InvalidReportName(String),

    /// Any other I/O failure.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Shorthand for results that fail with [`Error`].
pub type Result<T> = std::result::Result<T, Error>;
