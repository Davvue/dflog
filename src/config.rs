use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;
use crate::hash::HashAlgorithm;

pub const CONFIG_FILE: &str = "config.toml";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all="kebab-case")]
pub struct Config {
    pub case: CaseConfig,
    pub hash: HashConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all="kebab-case")]
pub struct CaseConfig {
    pub examiner: String,
    pub case_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all="kebab-case")]
pub struct HashConfig {
    pub algorithm: HashAlgorithm
}

impl Config {
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

    /// Writes the config, refusing to overwrite an existing file.
    pub fn save_new(&self, path: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::AlreadyExists => {
                    Error::AlreadyInitialized(path.to_path_buf())
                }
                _ => Error::Io(e),
            })?;
        file.write_all(text.as_bytes())?;
        Ok(())
    }
}