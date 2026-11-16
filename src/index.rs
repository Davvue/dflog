use crate::error::{Error, Result};
use crate::hash::HashAlgorithm;
use crate::journal::Digest;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{FileType, Metadata};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

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
    pub digest: Digest,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Stat {
    pub size: u64,
    pub mtime: [i64; 2], // [secs, nsecs]
    pub ctime: [i64; 2],
    pub dev: u64, // Unambiguous inode across mounts
    pub ino: u64,
}

impl Stat {
    fn from_metadata(m: &Metadata) -> Self {
        Stat {
            size: m.size(),
            mtime: [m.mtime(), m.mtime_nsec()],
            ctime: [m.ctime(), m.ctime_nsec()],
            dev: m.dev(),
            ino: m.ino(),
        }
    }
}

impl Index {
    pub fn snapshot(&self, root: &Path, scope: &Path, alg: HashAlgorithm) -> Result<Index> {
        let taken_at = jiff::Timestamp::now().as_second();
        let mut entries = BTreeMap::new();

        for item in WalkDir::new(root.join(scope)).follow_links(false) {
            let item = item.map_err(std::io::Error::from)?;
            let file_type = item.file_type();
            if file_type.is_dir() {
                continue;
            }

            let rel = item
                .path()
                .strip_prefix(root)
                .expect("walk stays under root");
            if rel.to_str().is_none() {
                return Err(Error::NonUtf8Path(item.path().to_path_buf()));
            }

            let meta = item.metadata().map_err(std::io::Error::from)?;
            let stat = Stat::from_metadata(&meta);

            let entry = match self.entries.get(rel) {
                Some(old) if self.reusable(old, &stat) => old.clone(),
                _ => IndexEntry {
                    digest: digest_of(item.path(), file_type, &stat, alg)?,
                    stat,
                },
            };
            entries.insert(rel.to_path_buf(), entry);
        }

        Ok(Index { taken_at, entries })
    }

    fn reusable(&self, old: &IndexEntry, now: &Stat) -> bool {
        !matches!(old.digest, Digest::Error(_))
            && old.stat == *now
            && old.stat.mtime[0] < self.taken_at
            && old.stat.ctime[0] < self.taken_at
    }
}

/// Digest for one non-directory entry. `Err` only for problems that must stop
/// the snapshot (a non-UTF-8 symlink target); per-file failures are recorded
/// as `Digest::Error`.
fn digest_of(path: &Path, file_type: FileType, stat: &Stat, alg: HashAlgorithm) -> Result<Digest> {
    if file_type.is_symlink() {
        return match std::fs::read_link(path) {
            Ok(target) if target.to_str().is_none() => Err(Error::NonUtf8Path(path.to_path_buf())),
            Ok(target) => Ok(Digest::Symlink(target)),
            Err(e) => Ok(Digest::Error(e.to_string())),
        };
    }

    if !file_type.is_file() {
        return Ok(Digest::Error(format!(
            "not a regular file: {}",
            kind(file_type)
        )));
    }

    Ok(hash_file(path, stat, alg).unwrap_or_else(|e| Digest::Error(e.to_string())))
}

/// Hashes the file at `path`, provided it is still the file `stat` describes.
fn hash_file(path: &Path, stat: &Stat, alg: HashAlgorithm) -> std::io::Result<Digest> {
    let file = std::fs::File::open(path)?;
    let opened = file.metadata()?;
    if (opened.dev(), opened.ino()) != (stat.dev, stat.ino) {
        return Ok(Digest::Error(
            "file was replaced while being indexed".into(),
        ));
    }
    Ok(Digest::Hash(alg.prefixed_digest(file)?))
}

fn kind(t: FileType) -> &'static str {
    if t.is_fifo() {
        "fifo"
    } else if t.is_socket() {
        "socket"
    } else if t.is_block_device() {
        "block_device"
    } else if t.is_char_device() {
        "char_device"
    } else {
        "unknown"
    }
}
