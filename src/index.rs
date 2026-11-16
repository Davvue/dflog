use std::collections::BTreeMap;
use std::path::PathBuf;
use serde::{Serialize, Deserialize};
use crate::journal::Digest;

pub const INDEX_FILE: &str = "index.json";

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct Index {
    /// Unix seconds, taken *before* the walk started
    pub taken_at: i64,
    pub entries: BTreeMap<PathBuf, IndexEntry>, // keys relative to workspace root
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    #[serde(flatten)]
    pub stat: Stat,
    #[serde(flatten)]
    pub digest: Digest
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Stat {
    pub size: u64,
    pub mtime: [i64; 2], // [secs, nsecs]
    pub ctime: [i64; 2],
    pub dev: u64, // Unambiguous inode across mounts
    pub ino: u64,
}