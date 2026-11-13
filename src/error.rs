use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not a dflog workspace (or any parent up to /): {0}")]
    WorkspaceNotFound(PathBuf),

    #[error("workspace already initialized at {0}")]
    AlreadyInitialized(PathBuf),

    #[error("could not read config {path}: {source}")]
    ConfigRead {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("invalid config {path}: {source}")]
    ConfigParse {
        path: PathBuf,
        source: toml::de::Error,
    },

    #[error("could not serialize config: {0}")]
    ConfigSerialize(#[from] toml::ser::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;