use crate::error::{Error, Result};
use crate::hash::HashAlgorithm;
use crate::journal::{Change, Digest, FileChange, FileStat};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{FileType, Metadata};
use std::io::Write;
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

impl IndexEntry {
    /// What the journal records for this entry.
    fn file_stat(&self) -> FileStat {
        FileStat {
            size: self.stat.size,
            digest: self.digest.clone(),
        }
    }

    /// Whether the content differs between `self` and `other`.
    ///
    /// With a hash or symlink target on both sides, those are compared and
    /// the stat is ignored. If either side could not be read there is no
    /// content to compare, so any difference in stat or error text counts
    /// as a change.
    fn content_changed(&self, other: &IndexEntry) -> bool {
        match (&self.digest, &other.digest) {
            (Digest::Error(_), _) | (_, Digest::Error(_)) => self != other,
            (a, b) => a != b,
        }
    }
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
            let item = match item {
                Ok(item) => item,
                Err(e) => {
                    let (rel, entry) = unreadable_dir(root, e)?;
                    entries.insert(rel, entry);
                    continue;
                }
            };
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
                Some(old) if self.reusable(old, &stat, alg) => old.clone(),
                _ => IndexEntry {
                    digest: digest_of(item.path(), file_type, &stat, alg)?,
                    stat,
                },
            };
            entries.insert(rel.to_path_buf(), entry);
        }

        Ok(Index { taken_at, entries })
    }

    /// Changes from `self` (the earlier snapshot) to `after`, sorted by path.
    ///
    /// A file only in `after` is `Created`, one only in `self` is `Deleted`,
    /// and one in both is `Modified` if its content changed (see
    /// [`IndexEntry::content_changed`]). Metadata-only changes such as `touch`
    /// are not reported. Never produces `Unchanged`; that is for files named
    /// explicitly as artifacts.
    pub fn diff(&self, after: &Index) -> Vec<FileChange> {
        let paths: BTreeSet<&PathBuf> = self.entries.keys().chain(after.entries.keys()).collect();

        paths
            .into_iter()
            .filter_map(|path| {
                let change = match (self.entries.get(path), after.entries.get(path)) {
                    (None, Some(new)) => Change::Created {
                        after: new.file_stat(),
                    },
                    (Some(old), None) => Change::Deleted {
                        before: old.file_stat(),
                    },
                    (Some(old), Some(new)) if old.content_changed(new) => Change::Modified {
                        before: old.file_stat(),
                        after: new.file_stat(),
                    },
                    _ => return None,
                };
                Some(FileChange {
                    path: path.clone(),
                    change,
                })
            })
            .collect()
    }

    /// Whether `old`'s digest can be kept for a file whose metadata is now
    /// `now`, instead of rehashing it.
    ///
    /// Never for errors (the file may be readable now), never for a hash made
    /// with a different algorithm than `alg` (otherwise one snapshot would mix
    /// algorithms and a later diff would compare hashes that can never be
    /// equal), and never for racy entries whose mtime or ctime is not strictly
    /// before the index was taken.
    fn reusable(&self, old: &IndexEntry, now: &Stat, alg: HashAlgorithm) -> bool {
        let digest_ok = match &old.digest {
            Digest::Hash(h) => h
                .strip_prefix(alg.name())
                .is_some_and(|rest| rest.starts_with(':')),
            Digest::Symlink(_) => true,
            Digest::Error(_) => false,
        };
        digest_ok
            && old.stat == *now
            && old.stat.mtime[0] < self.taken_at
            && old.stat.ctime[0] < self.taken_at
    }

    /// Reads the index at `path`.
    ///
    /// Returns `Ok(None)` if there is no index file, so the caller decides
    /// what that means. Treating it as an empty index would report every file
    /// as created. Fails with [`Error::IndexCorrupt`] if the file does not
    /// parse.
    pub fn load(path: &Path) -> Result<Option<Index>> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|source| Error::IndexCorrupt {
                path: path.to_path_buf(),
                source,
            })
    }

    /// Writes the index to `path`, replacing any existing index atomically.
    ///
    /// The index is written to `<path>.tmp`, synced, and renamed over `path`;
    /// then the parent directory is synced so the rename itself survives a
    /// crash. A reader therefore sees either the old or the new index, never
    /// a partial one. Callers must not save concurrently: both would write
    /// the same temp file.
    pub fn save(&self, path: &Path) -> Result<()> {
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);

        let bytes = serde_json::to_vec(self)?;
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);

        std::fs::rename(&tmp, path)?;

        let dir = match path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir,
            _ => Path::new("."),
        };
        std::fs::File::open(dir)?.sync_all()?;
        Ok(())
    }
}

/// Turns a walk error into an index entry for the directory that could not be
/// read, so one unreadable directory is recorded like an unreadable file
/// instead of aborting the whole snapshot. Its contents are not visible, but
/// a change to its stat (e.g. its mtime, when entries are added or removed)
/// still shows up as `Modified`.
///
/// Errors at the scope root itself (e.g. `working/` missing) and errors
/// without a path still fail the snapshot.
fn unreadable_dir(root: &Path, e: walkdir::Error) -> Result<(PathBuf, IndexEntry)> {
    let path = match e.path() {
        Some(path) if e.depth() > 0 => path.to_path_buf(),
        _ => return Err(std::io::Error::from(e).into()),
    };
    let rel = path
        .strip_prefix(root)
        .expect("walk stays under root")
        .to_path_buf();
    if rel.to_str().is_none() {
        return Err(Error::NonUtf8Path(path));
    }
    let meta = std::fs::symlink_metadata(&path)?;
    let entry = IndexEntry {
        stat: Stat::from_metadata(&meta),
        digest: Digest::Error(format!("could not read directory: {e}")),
    };
    Ok((rel, entry))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File, FileTimes};
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::time::{Duration, SystemTime};

    const ALG: HashAlgorithm = HashAlgorithm::Sha256;
    const SCOPE: &str = "working";

    // ---- fixtures -------------------------------------------------------

    /// A temp workspace root with an empty `working/` directory.
    fn root() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join(SCOPE)).unwrap();
        tmp
    }

    fn write(root: &Path, rel: &str, content: &[u8]) {
        let path = root.join(SCOPE).join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    /// Workspace-relative key as stored in the index.
    fn key(rel: &str) -> PathBuf {
        Path::new(SCOPE).join(rel)
    }

    fn snap(prev: &Index, root: &Path) -> Index {
        prev.snapshot(root, Path::new(SCOPE), ALG).unwrap()
    }

    fn first(root: &Path) -> Index {
        snap(&Index::default(), root)
    }

    /// Pretends `index` was taken long ago, so none of its entries are racy
    /// and unchanged ones are eligible for reuse.
    fn aged(mut index: Index) -> Index {
        index.taken_at += 3600;
        index
    }

    fn sha256(content: &[u8]) -> Digest {
        Digest::Hash(ALG.prefixed_digest(content).unwrap())
    }

    fn set_mtime(path: &Path, t: SystemTime) {
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(t))
            .unwrap();
    }

    fn mtime(path: &Path) -> SystemTime {
        fs::metadata(path).unwrap().modified().unwrap()
    }

    /// Permission checks don't apply to root, so permission tests are
    /// skipped when running as root.
    fn is_root() -> bool {
        let probe = tempfile::NamedTempFile::new().unwrap();
        fs::set_permissions(probe.path(), fs::Permissions::from_mode(0o000)).unwrap();
        File::open(probe.path()).is_ok()
    }

    fn entry(size: u64, digest: Digest) -> IndexEntry {
        IndexEntry {
            stat: Stat {
                size,
                mtime: [1_700_000_000, 0],
                ctime: [1_700_000_000, 0],
                dev: 1,
                ino: 42,
            },
            digest,
        }
    }

    fn index(entries: Vec<(&str, IndexEntry)>) -> Index {
        Index {
            taken_at: 1_800_000_000,
            entries: entries
                .into_iter()
                .map(|(p, e)| (PathBuf::from(p), e))
                .collect(),
        }
    }

    fn hash(s: &str) -> Digest {
        Digest::Hash(format!("sha256:{s}"))
    }

    // ---- diff (pure) ----------------------------------------------------

    #[test]
    fn diff_of_identical_indexes_is_empty() {
        let a = index(vec![
            ("a", entry(1, hash("aa"))),
            ("b", entry(2, hash("bb"))),
        ]);
        assert!(a.diff(&a.clone()).is_empty());
        assert!(Index::default().diff(&Index::default()).is_empty());
    }

    #[test]
    fn diff_reports_created_modified_deleted() {
        let before = index(vec![
            ("deleted", entry(1, hash("d"))),
            ("modified", entry(2, hash("m1"))),
            ("same", entry(3, hash("s"))),
        ]);
        let after = index(vec![
            ("created", entry(4, hash("c"))),
            ("modified", entry(5, hash("m2"))),
            ("same", entry(3, hash("s"))),
        ]);

        let changes = before.diff(&after);
        let stat = |size, d| FileStat { size, digest: d };
        assert_eq!(
            changes,
            vec![
                FileChange {
                    path: "created".into(),
                    change: Change::Created {
                        after: stat(4, hash("c"))
                    },
                },
                FileChange {
                    path: "deleted".into(),
                    change: Change::Deleted {
                        before: stat(1, hash("d"))
                    },
                },
                FileChange {
                    path: "modified".into(),
                    change: Change::Modified {
                        before: stat(2, hash("m1")),
                        after: stat(5, hash("m2")),
                    },
                },
            ]
        );
    }

    #[test]
    fn diff_is_sorted_by_path() {
        let after = index(vec![
            ("z", entry(1, hash("1"))),
            ("a/b", entry(1, hash("2"))),
            ("m", entry(1, hash("3"))),
        ]);
        let paths: Vec<_> = Index::default()
            .diff(&after)
            .into_iter()
            .map(|c| c.path)
            .collect();
        assert_eq!(paths, vec![PathBuf::from("a/b"), "m".into(), "z".into()]);
    }

    #[test]
    fn diff_ignores_stat_only_changes_when_hash_matches() {
        let before = index(vec![("f", entry(1, hash("x")))]);
        let mut after = before.clone();
        let e = after.entries.get_mut(Path::new("f")).unwrap();
        e.stat.mtime = [1_900_000_000, 5];
        e.stat.ctime = [1_900_000_000, 5];
        e.stat.ino = 99;
        assert!(before.diff(&after).is_empty());
    }

    #[test]
    fn diff_reports_symlink_target_change() {
        let before = index(vec![("l", entry(1, Digest::Symlink("a".into())))]);
        let after = index(vec![("l", entry(1, Digest::Symlink("b".into())))]);
        assert!(matches!(
            before.diff(&after)[..],
            [FileChange {
                change: Change::Modified { .. },
                ..
            }]
        ));
    }

    #[test]
    fn diff_reports_file_replaced_by_symlink() {
        let before = index(vec![("f", entry(1, hash("x")))]);
        let after = index(vec![("f", entry(1, Digest::Symlink("x".into())))]);
        assert_eq!(before.diff(&after).len(), 1);
    }

    #[test]
    fn diff_with_error_on_one_side_is_modified() {
        let ok = index(vec![("f", entry(1, hash("x")))]);
        let err = index(vec![("f", entry(1, Digest::Error("denied".into())))]);
        assert_eq!(ok.diff(&err).len(), 1);
        assert_eq!(err.diff(&ok).len(), 1);
    }

    #[test]
    fn diff_with_identical_errors_is_unchanged() {
        let err = index(vec![("f", entry(1, Digest::Error("denied".into())))]);
        assert!(err.diff(&err.clone()).is_empty());
    }

    #[test]
    fn diff_with_errors_reports_stat_change() {
        // No content to compare, so any stat change has to count.
        let before = index(vec![("f", entry(1, Digest::Error("denied".into())))]);
        let mut after = before.clone();
        after.entries.get_mut(Path::new("f")).unwrap().stat.mtime[1] = 1;
        assert_eq!(before.diff(&after).len(), 1);
    }

    #[test]
    fn diff_with_errors_reports_error_text_change() {
        let before = index(vec![("f", entry(1, Digest::Error("denied".into())))]);
        let after = index(vec![("f", entry(1, Digest::Error("io error".into())))]);
        assert_eq!(before.diff(&after).len(), 1);
    }

    #[test]
    fn diff_never_produces_unchanged() {
        let a = index(vec![("f", entry(1, hash("x")))]);
        assert!(
            a.diff(&a.clone())
                .iter()
                .all(|c| !matches!(c.change, Change::Unchanged { .. }))
        );
    }

    // ---- snapshot: walking ----------------------------------------------

    #[test]
    fn snapshot_of_empty_scope_is_empty() {
        let tmp = root();
        assert!(first(tmp.path()).entries.is_empty());
    }

    #[test]
    fn snapshot_hashes_files_with_relative_keys() {
        let tmp = root();
        write(tmp.path(), "a.txt", b"abc");
        write(tmp.path(), "sub/dir/b.bin", b"");

        let idx = first(tmp.path());
        assert_eq!(
            idx.entries.keys().collect::<Vec<_>>(),
            vec![&key("a.txt"), &key("sub/dir/b.bin")]
        );
        let a = &idx.entries[&key("a.txt")];
        assert_eq!(a.digest, sha256(b"abc"));
        assert_eq!(a.stat.size, 3);
        assert_eq!(idx.entries[&key("sub/dir/b.bin")].digest, sha256(b""));
    }

    #[test]
    fn snapshot_skips_directories_even_empty_ones() {
        let tmp = root();
        fs::create_dir_all(tmp.path().join(SCOPE).join("empty/nested")).unwrap();
        assert!(first(tmp.path()).entries.is_empty());
    }

    #[test]
    fn snapshot_only_walks_scope() {
        let tmp = root();
        fs::create_dir(tmp.path().join(".dflog")).unwrap();
        fs::write(tmp.path().join(".dflog/journal.jsonl"), b"{}").unwrap();
        fs::write(tmp.path().join("outside.txt"), b"x").unwrap();
        write(tmp.path(), "inside.txt", b"y");

        let idx = first(tmp.path());
        assert_eq!(
            idx.entries.keys().collect::<Vec<_>>(),
            vec![&key("inside.txt")]
        );
    }

    #[test]
    fn snapshot_fails_if_scope_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(
            Index::default()
                .snapshot(tmp.path(), Path::new(SCOPE), ALG)
                .is_err()
        );
    }

    #[test]
    fn snapshot_records_taken_at_before_walk() {
        let tmp = root();
        let before = jiff::Timestamp::now().as_second();
        let idx = first(tmp.path());
        let after = jiff::Timestamp::now().as_second();
        assert!((before..=after).contains(&idx.taken_at));
    }

    #[test]
    fn snapshot_uses_requested_algorithm() {
        let tmp = root();
        write(tmp.path(), "f", b"abc");
        let idx = Index::default()
            .snapshot(tmp.path(), Path::new(SCOPE), HashAlgorithm::Sha512)
            .unwrap();
        let Digest::Hash(h) = &idx.entries[&key("f")].digest else {
            panic!("expected hash");
        };
        assert!(h.starts_with("sha512:"));
    }

    #[test]
    fn snapshot_hashes_large_files() {
        let tmp = root();
        let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        write(tmp.path(), "big", &big);
        assert_eq!(first(tmp.path()).entries[&key("big")].digest, sha256(&big));
    }

    // ---- snapshot: symlinks ---------------------------------------------

    #[test]
    fn symlink_records_target_and_is_not_followed() {
        let tmp = root();
        write(tmp.path(), "real/secret", b"data");
        let scope = tmp.path().join(SCOPE);
        symlink("real/secret", scope.join("file-link")).unwrap();
        symlink("real", scope.join("dir-link")).unwrap();

        let idx = first(tmp.path());
        assert_eq!(
            idx.entries[&key("file-link")].digest,
            Digest::Symlink("real/secret".into())
        );
        assert_eq!(
            idx.entries[&key("dir-link")].digest,
            Digest::Symlink("real".into())
        );
        // The directory behind dir-link is not walked a second time.
        assert!(!idx.entries.contains_key(&key("dir-link/secret")));
    }

    #[test]
    fn symlink_out_of_workspace_is_not_followed() {
        let tmp = root();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("x"), b"x").unwrap();
        symlink(outside.path(), tmp.path().join(SCOPE).join("ext")).unwrap();

        let idx = first(tmp.path());
        assert_eq!(idx.entries.len(), 1);
        assert_eq!(
            idx.entries[&key("ext")].digest,
            Digest::Symlink(outside.path().into())
        );
    }

    #[test]
    fn dangling_symlink_is_recorded() {
        let tmp = root();
        symlink("nowhere", tmp.path().join(SCOPE).join("dangling")).unwrap();
        assert_eq!(
            first(tmp.path()).entries[&key("dangling")].digest,
            Digest::Symlink("nowhere".into())
        );
    }

    #[test]
    fn symlink_retarget_is_modified() {
        let tmp = root();
        let link = tmp.path().join(SCOPE).join("l");
        symlink("a", &link).unwrap();
        let before = first(tmp.path());
        fs::remove_file(&link).unwrap();
        symlink("b", &link).unwrap();
        let after = snap(&aged(before.clone()), tmp.path());

        assert_eq!(
            before.diff(&after),
            vec![FileChange {
                path: key("l"),
                change: Change::Modified {
                    before: FileStat {
                        size: 1,
                        digest: Digest::Symlink("a".into())
                    },
                    after: FileStat {
                        size: 1,
                        digest: Digest::Symlink("b".into())
                    },
                },
            }]
        );
    }

    // ---- snapshot + diff: end to end ------------------------------------

    #[test]
    fn create_modify_delete_end_to_end() {
        let tmp = root();
        write(tmp.path(), "keep", b"keep");
        write(tmp.path(), "modify", b"old");
        write(tmp.path(), "delete", b"gone");
        let before = first(tmp.path());

        write(tmp.path(), "modify", b"new content");
        fs::remove_file(tmp.path().join(SCOPE).join("delete")).unwrap();
        write(tmp.path(), "new/created", b"hi");
        let after = snap(&aged(before.clone()), tmp.path());

        let changes = before.diff(&after);
        let summary: Vec<_> = changes
            .iter()
            .map(|c| {
                let op = match &c.change {
                    Change::Created { .. } => "created",
                    Change::Modified { .. } => "modified",
                    Change::Deleted { .. } => "deleted",
                    Change::Unchanged { .. } => "unchanged",
                };
                (c.path.clone(), op)
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                (key("delete"), "deleted"),
                (key("modify"), "modified"),
                (key("new/created"), "created"),
            ]
        );
        let Change::Modified {
            before: b,
            after: a,
        } = &changes[1].change
        else {
            unreachable!()
        };
        assert_eq!(b.digest, sha256(b"old"));
        assert_eq!(a.digest, sha256(b"new content"));
        assert_eq!((b.size, a.size), (3, 11));
    }

    #[test]
    fn rename_is_delete_plus_create() {
        let tmp = root();
        write(tmp.path(), "a", b"x");
        let before = first(tmp.path());
        let scope = tmp.path().join(SCOPE);
        fs::rename(scope.join("a"), scope.join("b")).unwrap();
        let after = snap(&aged(before.clone()), tmp.path());

        let changes = before.diff(&after);
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].path, key("a"));
        assert!(matches!(changes[0].change, Change::Deleted { .. }));
        assert_eq!(changes[1].path, key("b"));
        assert!(matches!(changes[1].change, Change::Created { .. }));
    }

    #[test]
    fn touch_is_not_reported() {
        let tmp = root();
        write(tmp.path(), "f", b"same");
        let before = first(tmp.path());
        set_mtime(
            &tmp.path().join(SCOPE).join("f"),
            SystemTime::now() + Duration::from_secs(100),
        );
        let after = snap(&aged(before.clone()), tmp.path());

        assert_ne!(
            before.entries[&key("f")].stat,
            after.entries[&key("f")].stat
        );
        assert!(before.diff(&after).is_empty());
    }

    #[test]
    fn same_size_in_place_overwrite_with_restored_mtime_is_modified() {
        // The case unzip / tar -x / cp -p create: same size, old mtime.
        let tmp = root();
        let path = tmp.path().join(SCOPE).join("f");
        write(tmp.path(), "f", b"aaaa");
        let old_mtime = SystemTime::now() - Duration::from_secs(86_400);
        set_mtime(&path, old_mtime);
        let before = first(tmp.path());

        fs::write(&path, b"bbbb").unwrap(); // same inode, same size
        set_mtime(&path, old_mtime);
        assert_eq!(mtime(&path), old_mtime);
        let after = snap(&aged(before.clone()), tmp.path());

        let (b, a) = (&before.entries[&key("f")], &after.entries[&key("f")]);
        assert_eq!(
            (b.stat.size, b.stat.mtime, b.stat.ino),
            (a.stat.size, a.stat.mtime, a.stat.ino)
        );
        assert_eq!(a.digest, sha256(b"bbbb"));
        assert_eq!(before.diff(&after).len(), 1);
    }

    #[test]
    fn same_size_replace_via_rename_with_restored_mtime_is_modified() {
        let tmp = root();
        let scope = tmp.path().join(SCOPE);
        let path = scope.join("f");
        write(tmp.path(), "f", b"aaaa");
        let old_mtime = SystemTime::now() - Duration::from_secs(86_400);
        set_mtime(&path, old_mtime);
        let before = first(tmp.path());

        let tmp_file = tmp.path().join("staging");
        fs::write(&tmp_file, b"bbbb").unwrap();
        set_mtime(&tmp_file, old_mtime);
        fs::rename(&tmp_file, &path).unwrap(); // new inode
        let after = snap(&aged(before.clone()), tmp.path());

        assert_eq!(after.entries[&key("f")].digest, sha256(b"bbbb"));
        assert_eq!(before.diff(&after).len(), 1);
    }

    #[test]
    fn content_change_detected_without_aging() {
        // Racy entries are always rehashed, so a change in the same second
        // as the previous snapshot is still caught.
        let tmp = root();
        write(tmp.path(), "f", b"one");
        let before = first(tmp.path());
        write(tmp.path(), "f", b"two");
        let after = snap(&before, tmp.path());
        assert_eq!(before.diff(&after).len(), 1);
    }

    // ---- snapshot: hash reuse -------------------------------------------

    /// Replaces the stored digest of `rel` with a marker that no real hash
    /// equals, so a test can tell whether the next snapshot reused it.
    fn plant(index: &mut Index, rel: &str, marker: &str) {
        index.entries.get_mut(&key(rel)).unwrap().digest = Digest::Hash(marker.into());
    }

    #[test]
    fn unchanged_file_reuses_stored_hash() {
        let tmp = root();
        write(tmp.path(), "f", b"x");
        let mut prev = aged(first(tmp.path()));
        plant(&mut prev, "f", "sha256:planted");

        let next = snap(&prev, tmp.path());
        assert_eq!(
            next.entries[&key("f")].digest,
            Digest::Hash("sha256:planted".into())
        );
    }

    #[test]
    fn changed_stat_is_rehashed() {
        let tmp = root();
        write(tmp.path(), "f", b"x");
        let mut prev = aged(first(tmp.path()));
        plant(&mut prev, "f", "sha256:planted");
        set_mtime(
            &tmp.path().join(SCOPE).join("f"),
            SystemTime::UNIX_EPOCH + Duration::from_secs(1),
        );

        let next = snap(&prev, tmp.path());
        assert_eq!(next.entries[&key("f")].digest, sha256(b"x"));
    }

    #[test]
    fn racy_mtime_is_rehashed() {
        let tmp = root();
        write(tmp.path(), "f", b"x");
        let mut prev = first(tmp.path());
        plant(&mut prev, "f", "sha256:planted");
        // mtime in the same second the index was taken: could have been
        // modified again after the walk without the stat changing.
        prev.taken_at = prev.entries[&key("f")].stat.mtime[0];

        let next = snap(&prev, tmp.path());
        assert_eq!(next.entries[&key("f")].digest, sha256(b"x"));
    }

    #[test]
    fn racy_mtime_after_taken_at_is_rehashed() {
        let tmp = root();
        write(tmp.path(), "f", b"x");
        let mut prev = first(tmp.path());
        plant(&mut prev, "f", "sha256:planted");
        prev.taken_at = prev.entries[&key("f")].stat.mtime[0] - 10;

        let next = snap(&prev, tmp.path());
        assert_eq!(next.entries[&key("f")].digest, sha256(b"x"));
    }

    #[test]
    fn racy_ctime_is_rehashed_even_with_old_mtime() {
        let tmp = root();
        let path = tmp.path().join(SCOPE).join("f");
        write(tmp.path(), "f", b"x");
        set_mtime(&path, SystemTime::now() - Duration::from_secs(86_400));
        let mut prev = first(tmp.path());
        plant(&mut prev, "f", "sha256:planted");
        let stat = &prev.entries[&key("f")].stat;
        assert!(stat.mtime[0] < stat.ctime[0]);
        prev.taken_at = stat.ctime[0];

        let next = snap(&prev, tmp.path());
        assert_eq!(next.entries[&key("f")].digest, sha256(b"x"));
    }

    #[test]
    fn hash_from_other_algorithm_is_not_reused() {
        let tmp = root();
        write(tmp.path(), "f", b"x");
        let prev = aged(first(tmp.path())); // sha256

        let next = prev
            .snapshot(tmp.path(), Path::new(SCOPE), HashAlgorithm::Sha512)
            .unwrap();
        assert_eq!(
            next.entries[&key("f")].digest,
            Digest::Hash(HashAlgorithm::Sha512.prefixed_digest(&b"x"[..]).unwrap())
        );
    }

    #[test]
    fn algorithm_prefix_must_match_exactly() {
        // "sha256" must not match a hypothetical "sha256x:..." prefix.
        let tmp = root();
        write(tmp.path(), "f", b"x");
        let mut prev = aged(first(tmp.path()));
        plant(&mut prev, "f", "sha256x:planted");

        let next = snap(&prev, tmp.path());
        assert_eq!(next.entries[&key("f")].digest, sha256(b"x"));
    }

    #[test]
    fn symlink_is_reused_across_algorithms() {
        let tmp = root();
        symlink("t", tmp.path().join(SCOPE).join("l")).unwrap();
        let mut prev = aged(first(tmp.path()));
        prev.entries.get_mut(&key("l")).unwrap().digest = Digest::Symlink("planted".into());

        let next = prev
            .snapshot(tmp.path(), Path::new(SCOPE), HashAlgorithm::Sha512)
            .unwrap();
        assert_eq!(
            next.entries[&key("l")].digest,
            Digest::Symlink("planted".into())
        );
    }

    #[test]
    fn error_digest_is_never_reused() {
        let tmp = root();
        write(tmp.path(), "f", b"x");
        let mut prev = aged(first(tmp.path()));
        prev.entries.get_mut(&key("f")).unwrap().digest = Digest::Error("denied".into());

        let next = snap(&prev, tmp.path());
        assert_eq!(next.entries[&key("f")].digest, sha256(b"x"));
    }

    #[test]
    fn deleted_files_are_dropped_from_next_snapshot() {
        let tmp = root();
        write(tmp.path(), "f", b"x");
        let prev = aged(first(tmp.path()));
        fs::remove_file(tmp.path().join(SCOPE).join("f")).unwrap();
        assert!(snap(&prev, tmp.path()).entries.is_empty());
    }

    // ---- snapshot: unreadable and special files -------------------------

    #[test]
    fn unreadable_file_is_recorded_as_error() {
        if is_root() {
            return;
        }
        let tmp = root();
        let path = tmp.path().join(SCOPE).join("locked");
        write(tmp.path(), "locked", b"secret");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();

        let idx = first(tmp.path());
        let e = &idx.entries[&key("locked")];
        assert!(matches!(&e.digest, Digest::Error(_)), "{:?}", e.digest);
        assert_eq!(e.stat.size, 6);

        // Becoming readable again is a change, and is rehashed even though
        // the content and mtime are unchanged.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let next = snap(&aged(idx.clone()), tmp.path());
        assert_eq!(next.entries[&key("locked")].digest, sha256(b"secret"));
        assert_eq!(idx.diff(&next).len(), 1);
    }

    #[test]
    fn unreadable_directory_is_recorded_not_fatal() {
        if is_root() {
            return;
        }
        let tmp = root();
        write(tmp.path(), "ok.txt", b"ok");
        write(tmp.path(), "locked/inner.txt", b"hidden");
        let dir = tmp.path().join(SCOPE).join("locked");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o000)).unwrap();

        let result = Index::default().snapshot(tmp.path(), Path::new(SCOPE), ALG);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        let idx = result.unwrap();

        assert_eq!(idx.entries[&key("ok.txt")].digest, sha256(b"ok"));
        let Digest::Error(msg) = &idx.entries[&key("locked")].digest else {
            panic!("expected error for unreadable directory");
        };
        assert!(msg.contains("could not read directory"), "{msg}");
        assert!(!idx.entries.contains_key(&key("locked/inner.txt")));
    }

    #[test]
    fn unreadable_directory_becoming_readable_reports_contents() {
        if is_root() {
            return;
        }
        let tmp = root();
        write(tmp.path(), "locked/inner.txt", b"hidden");
        let dir = tmp.path().join(SCOPE).join("locked");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o000)).unwrap();
        let result = first_result(tmp.path());
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        let before = result.unwrap();

        let after = snap(&aged(before.clone()), tmp.path());
        let changes = before.diff(&after);
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].path, key("locked"));
        assert!(matches!(changes[0].change, Change::Deleted { .. }));
        assert_eq!(changes[1].path, key("locked/inner.txt"));
        assert!(matches!(changes[1].change, Change::Created { .. }));
    }

    fn first_result(root: &Path) -> Result<Index> {
        Index::default().snapshot(root, Path::new(SCOPE), ALG)
    }

    #[test]
    fn unreadable_scope_root_is_fatal() {
        if is_root() {
            return;
        }
        let tmp = root();
        let scope = tmp.path().join(SCOPE);
        fs::set_permissions(&scope, fs::Permissions::from_mode(0o000)).unwrap();
        let result = first_result(tmp.path());
        fs::set_permissions(&scope, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(result.is_err());
    }

    #[test]
    fn socket_is_recorded_as_not_a_regular_file() {
        let tmp = root();
        let path = tmp.path().join(SCOPE).join("s");
        let _listener = std::os::unix::net::UnixListener::bind(&path).unwrap();

        let idx = first(tmp.path());
        assert_eq!(
            idx.entries[&key("s")].digest,
            Digest::Error("not a regular file: socket".into())
        );
    }

    #[test]
    fn fifo_is_recorded_without_blocking() {
        let tmp = root();
        let path = tmp.path().join(SCOPE).join("p");
        let status = std::process::Command::new("mkfifo").arg(&path).status();
        if !status.is_ok_and(|s| s.success()) {
            return; // mkfifo not available
        }
        let idx = first(tmp.path());
        assert_eq!(
            idx.entries[&key("p")].digest,
            Digest::Error("not a regular file: fifo".into())
        );
    }

    #[test]
    fn non_utf8_file_name_is_an_error() {
        use std::os::unix::ffi::OsStrExt;
        let tmp = root();
        let name = std::ffi::OsStr::from_bytes(b"bad\xff");
        if fs::write(tmp.path().join(SCOPE).join(name), b"x").is_err() {
            return; // filesystem rejects non-UTF-8 names (e.g. APFS)
        }
        assert!(matches!(
            first_result(tmp.path()),
            Err(Error::NonUtf8Path(_))
        ));
    }

    #[test]
    fn non_utf8_symlink_target_is_an_error() {
        use std::os::unix::ffi::OsStrExt;
        let tmp = root();
        let target = std::ffi::OsStr::from_bytes(b"bad\xff");
        symlink(target, tmp.path().join(SCOPE).join("l")).unwrap();
        assert!(matches!(
            first_result(tmp.path()),
            Err(Error::NonUtf8Path(_))
        ));
    }

    // ---- load / save ----------------------------------------------------

    #[test]
    fn save_then_load_round_trips() {
        let tmp = root();
        write(tmp.path(), "a", b"a");
        write(tmp.path(), "b/c", b"c");
        symlink("a", tmp.path().join(SCOPE).join("l")).unwrap();
        let mut idx = first(tmp.path());
        idx.entries
            .insert("err".into(), entry(0, Digest::Error("denied".into())));

        let path = tmp.path().join(INDEX_FILE);
        idx.save(&path).unwrap();
        assert_eq!(Index::load(&path).unwrap(), Some(idx));
    }

    #[test]
    fn load_missing_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(Index::load(&tmp.path().join(INDEX_FILE)).unwrap(), None);
    }

    #[test]
    fn load_garbage_is_corrupt() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(INDEX_FILE);
        for bad in [
            &b""[..],
            b"not json",
            b"{\"taken_at\": 1}",
            b"{\"taken_at\":1,\"entries\":{",
        ] {
            fs::write(&path, bad).unwrap();
            assert!(
                matches!(Index::load(&path), Err(Error::IndexCorrupt { .. })),
                "{:?}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[test]
    fn load_unreadable_is_io_error() {
        let tmp = tempfile::tempdir().unwrap();
        // A directory at the index path can't be read as a file.
        let path = tmp.path().join(INDEX_FILE);
        fs::create_dir(&path).unwrap();
        assert!(matches!(Index::load(&path), Err(Error::Io(_))));
    }

    #[test]
    fn save_replaces_existing_and_leaves_no_temp_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(INDEX_FILE);
        let a = index(vec![("a", entry(1, hash("a")))]);
        let b = index(vec![("b", entry(2, hash("b")))]);

        a.save(&path).unwrap();
        b.save(&path).unwrap();
        assert_eq!(Index::load(&path).unwrap(), Some(b));
        let names: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from(INDEX_FILE)]);
    }

    #[test]
    fn save_overwrites_stale_temp_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(INDEX_FILE);
        fs::write(
            tmp.path().join(format!("{INDEX_FILE}.tmp")),
            b"left over from a crash",
        )
        .unwrap();
        let a = index(vec![("a", entry(1, hash("a")))]);
        a.save(&path).unwrap();
        assert_eq!(Index::load(&path).unwrap(), Some(a));
        assert!(!tmp.path().join(format!("{INDEX_FILE}.tmp")).exists());
    }

    #[test]
    fn save_fails_if_directory_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("missing/index.json");
        assert!(Index::default().save(&path).is_err());
    }

    #[test]
    fn serialized_entry_is_flat() {
        let e = entry(3, hash("abc"));
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["size"], 3);
        assert_eq!(v["hash"], "sha256:abc");
        assert_eq!(v["ino"], 42);
        assert!(v.get("stat").is_none() && v.get("digest").is_none());
    }
}
