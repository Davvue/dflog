use crate::config::{CONFIG_FILE, Config};
use crate::error::{Error, Result};
use std::path::{Path, PathBuf};

pub const DFLOG_DIR: &str = ".dflog";
const HASH_DIR: &str = "hash";
const CMD_OUT_DIR: &str = "cmd-out";
const WORKING_DIR: &str = "working";

#[derive(Debug)]
pub struct Workspace {
    /// Directory that contains `.dflog`.
    pub root: PathBuf,
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

    /// Creates a new workspace at `root`. Fails if one already exists there.
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
        std::fs::create_dir_all(ws.hash_dir())?;
        std::fs::create_dir_all(ws.cmd_out_dir())?;
        ws.config.save_new(&config_path)?;
        Ok(ws)
    }

    pub fn dflog_dir(&self) -> PathBuf {
        self.root.join(DFLOG_DIR)
    }

    pub fn config_path(&self) -> PathBuf {
        config_path(&self.root)
    }

    pub fn hash_dir(&self) -> PathBuf {
        self.dflog_dir().join(HASH_DIR)
    }

    pub fn cmd_out_dir(&self) -> PathBuf {
        self.dflog_dir().join(CMD_OUT_DIR)
    }

    pub fn working_dir(&self) -> PathBuf {
        self.root.join(WORKING_DIR)
    }
}

fn config_path(root: &Path) -> PathBuf {
    root.join(DFLOG_DIR).join(CONFIG_FILE)
}

#[cfg(test)]
mod tests {
    use crate::hash::HashAlgorithm;
    use super::*;

    fn sample_config() -> Config {
        Config {
            case: crate::config::CaseConfig {
                examiner: "Jane Doe".into(),
                case_id: "2026-0001".into(),
            },
            hash: crate::config::HashConfig {
                algorithm: HashAlgorithm::Sha512
            }
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
        std::fs::write(ws.config_path(), "[case]\nexaminer = \"x\"\nbogus = 1\n").unwrap();
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
            ws.config_path(),
            "[case]\nexaminer = \"x\"\ncase-id = \"y\"\n",
        )
        .unwrap();
        assert!(matches!(
            Workspace::discover(tmp.path()),
            Err(Error::ConfigParse { .. })
        ));
    }
}