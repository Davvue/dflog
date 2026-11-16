//! The case workspace containing a `.dflog` directory.
//!
//! Layout:
//! ```text
//! <root>/
//! ├── .dflog/
//! │   ├── config.toml     case config, see crate::config
//! │   ├── index.json      file index to detect changes
//! │   ├── journal.jsonl   hash-chained journal, see crate::journal
//! │   └── cmd-out/        captured stdout/stderr of wrapped commands
//! └── working/            case files the examiner works on
//! ```

use crate::config::{CONFIG_FILE, Config};
use crate::error::{Error, Result};
use crate::index::INDEX_FILE;
use std::path::{Path, PathBuf};

/// Name of the directory that marks a workspace root and holds dflog's data.
pub const DFLOG_DIR: &str = ".dflog";
/// Directory for captured command output, inside [`DFLOG_DIR`].
const CMD_OUT_DIR: &str = "cmd-out";
/// Directory for case files, directly inside the workspace root.
const WORKING_DIR: &str = "working";

/// An opened workspace: where it is and the config it was loaded with.
#[derive(Debug)]
pub struct Workspace {
    /// Directory that contains `.dflog`.
    pub root: PathBuf,
    /// The config loaded from (or, for a new workspace, written to)
    /// `.dflog/config.toml`.
    pub config: Config,
}

// `discover` and some helpers are used by the upcoming non-init subcommands.
impl Workspace {
    /// Walks up from `start` until a directory containing `.dflog` is found,
    /// then loads its config. A broken config is an error; the search does
    /// not continue to parent workspaces.
    pub fn discover(start: &Path) -> Result<Self> {
        let start = start.canonicalize()?;
        let root = start
            .ancestors()
            .find(|dir| dir.join(DFLOG_DIR).is_dir())
            .ok_or_else(|| Error::WorkspaceNotFound(start.clone()))?
            .to_path_buf();

        let config = Config::load(&config_path(&root))?;
        Ok(Self { root, config })
    }

    /// Creates a new workspace at `root`: the `working/` and `.dflog/cmd-out/`
    /// directories and the config file. Fails with
    /// [`Error::AlreadyInitialized`] if `root` already has a config.
    pub fn create(root: &Path, config: Config) -> Result<Self> {
        let config_path = config_path(root);
        if config_path.exists() {
            return Err(Error::AlreadyInitialized(root.to_path_buf()));
        }

        let ws = Self {
            root: root.to_path_buf(),
            config,
        };
        std::fs::create_dir_all(ws.working_dir())?;
        std::fs::create_dir_all(ws.cmd_out_dir())?;
        ws.config.save_new(&config_path)?;
        Ok(ws)
    }

    /// `<root>/.dflog`
    pub fn dflog_dir(&self) -> PathBuf {
        self.root.join(DFLOG_DIR)
    }

    /// `<root>/.dflog/cmd-out`
    pub fn cmd_out_dir(&self) -> PathBuf {
        self.dflog_dir().join(CMD_OUT_DIR)
    }

    /// `<root>/.dflog/index.json`
    pub fn index_path(&self) -> PathBuf {
        self.dflog_dir().join(INDEX_FILE)
    }

    /// `<root>/working`
    pub fn working_dir(&self) -> PathBuf {
        self.root.join(WORKING_DIR)
    }

    /// Returns the working directory relative to the workspace root
    pub fn working_scope() -> &'static str {
        WORKING_DIR
    }
}

/// Config path for a workspace at `root`. A free function because
/// `Workspace::discover` and `Workspace::create` need it before a
/// `Workspace` exists.
fn config_path(root: &Path) -> PathBuf {
    root.join(DFLOG_DIR).join(CONFIG_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::HashAlgorithm;

    fn sample_config() -> Config {
        Config {
            case: crate::config::CaseConfig {
                examiner: "Jane Doe".into(),
                case_id: "2026-0001".into(),
            },
            hash: crate::config::HashConfig {
                algorithm: HashAlgorithm::Sha512,
            },
        }
    }

    #[test]
    fn create_then_discover_from_subdirectory() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = Workspace::create(tmp.path(), sample_config()).unwrap();

        let nested = ws.working_dir().join("a/b");
        std::fs::create_dir_all(&nested).unwrap();

        let found = Workspace::discover(&nested).unwrap();
        assert_eq!(found.root, tmp.path().canonicalize().unwrap());
        assert_eq!(found.config, sample_config());
    }

    #[test]
    fn discover_errors_outside_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(matches!(
            Workspace::discover(tmp.path()),
            Err(Error::WorkspaceNotFound(_))
        ));
    }

    #[test]
    fn create_twice_fails() {
        let tmp = tempfile::tempdir().unwrap();
        Workspace::create(tmp.path(), sample_config()).unwrap();
        assert!(matches!(
            Workspace::create(tmp.path(), sample_config()),
            Err(Error::AlreadyInitialized(_))
        ));
    }

    #[test]
    fn broken_config_is_reported_not_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = Workspace::create(tmp.path(), sample_config()).unwrap();
        std::fs::write(
            config_path(&ws.root),
            "[case]\nexaminer = \"x\"\nbogus = 1\n",
        )
        .unwrap();
        assert!(matches!(
            Workspace::discover(tmp.path()),
            Err(Error::ConfigParse { .. })
        ));
    }

    #[test]
    fn missing_hash_algorithm_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = Workspace::create(tmp.path(), sample_config()).unwrap();
        std::fs::write(
            config_path(&ws.root),
            "[case]\nexaminer = \"x\"\ncase-id = \"y\"\n",
        )
        .unwrap();
        assert!(matches!(
            Workspace::discover(tmp.path()),
            Err(Error::ConfigParse { .. })
        ));
    }
}
