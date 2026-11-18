//! The per-case configuration, stored as TOML-file in the workspace directory.
//!
//! Parsing is strict: unknown keys and missing keys are errors, so a typo in
//! the config is reported instead of silently falling back to a default.

use crate::error::{Error, Result};
use crate::hash::HashAlgorithm;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

/// File name of the config inside the `.dflog` directory.
pub const CONFIG_FILE: &str = "config.toml";

/// The whole config file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Config {
    /// The `[case]` table.
    pub case: CaseConfig,
    /// The `[hash]` table.
    pub hash: HashConfig,
    /// The `[report]` table
    #[serde(default)]
    pub report: ReportConfig,
}

/// Who is responsible for the case and how it is identified.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct CaseConfig {
    /// Name of the examiner responsible for the case. Copied into every
    /// journal entry.
    pub examiner: String,
    /// Case identifier, stored as `case-id` in TOML. Copied into every
    /// journal entry.
    pub case_id: String,
}

/// Hashing settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct HashConfig {
    /// Algorithm used for new hashes. Changing it later only affects new
    /// journal entries; existing hashes carry their own algorithm prefix.
    pub algorithm: HashAlgorithm,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct ReportConfig {
    /// Default name format for new reports, excluding the file extension
    pub name_format: String,
}

pub const DEFAULT_REPORT_NAME_FORMAT: &str = "%Y%m%dT%H%M%S%.3fZ";
impl Default for ReportConfig {
    fn default() -> Self {
        Self {
            name_format: DEFAULT_REPORT_NAME_FORMAT.into(),
        }
    }
}

impl Config {
    /// Reads and parses the config at `path`.
    ///
    /// Fails with [`Error::ConfigRead`] if the file cannot be read and with
    /// [`Error::ConfigParse`] if it is not valid, including unknown or missing
    /// keys.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|source| Error::ConfigRead {
            path: path.to_path_buf(),
            source,
        })?;
        toml::from_str(&text).map_err(|source| Error::ConfigParse {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Writes the config to `path`, refusing to overwrite an existing file.
    ///
    /// Fails with [`Error::AlreadyInitialized`] if `path` already exists.
    pub fn save_new(&self, path: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::AlreadyExists => Error::AlreadyInitialized(path.to_path_buf()),
                _ => Error::Io(e),
            })?;
        file.write_all(text.as_bytes())?;
        Ok(())
    }
}
