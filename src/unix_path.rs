//! Serde adapters that store paths with `/` as separator on every OS, so a
//! journal or index written on Windows reads the same as one from Unix.
//!
//! Paths in memory stay native; only the serialized form changes. On
//! Windows `\` becomes `/` when writing and `/` becomes `\` when reading.
//! Windows file names can't contain either character, so that is lossless.
//! On Unix paths are written as they are.
//!
//! Use with `#[serde(with = "crate::unix_path")]` on a `PathBuf` field and
//! `#[serde(with = "crate::unix_path::map")]` on a `BTreeMap<PathBuf, _>`.

use serde::de::Deserializer;
use serde::ser::{Error as _, Serializer};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::path::{Path, PathBuf};

pub fn serialize<S: Serializer>(path: &Path, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&to_unix(path).map_err(S::Error::custom)?)
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<PathBuf, D::Error> {
    String::deserialize(d).map(|s| from_unix(&s))
}

/// For `BTreeMap<PathBuf, V>`: the keys are stored like [`serialize`] does.
pub mod map {
    use super::*;
    use std::collections::BTreeMap;

    pub fn serialize<S, V>(map: &BTreeMap<PathBuf, V>, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        V: Serialize,
    {
        s.collect_map(map.iter().map(|(k, v)| (Key(k), v)))
    }

    pub fn deserialize<'de, D, V>(d: D) -> Result<BTreeMap<PathBuf, V>, D::Error>
    where
        D: Deserializer<'de>,
        V: Deserialize<'de>,
    {
        let raw = BTreeMap::<String, V>::deserialize(d)?;
        Ok(raw.into_iter().map(|(k, v)| (from_unix(&k), v)).collect())
    }

    struct Key<'a>(&'a Path);

    impl Serialize for Key<'_> {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            super::serialize(self.0, s)
        }
    }
}

/// `path` with `/` as separator. Fails for non-UTF-8 paths, like serde's own
/// `Path` impl.
fn to_unix(path: &Path) -> Result<Cow<'_, str>, &'static str> {
    let s = path
        .to_str()
        .ok_or("path contains invalid UTF-8 characters")?;
    if cfg!(windows) && s.contains('\\') {
        Ok(Cow::Owned(s.replace('\\', "/")))
    } else {
        Ok(Cow::Borrowed(s))
    }
}

/// The native path for a stored one.
fn from_unix(s: &str) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(s.replace('/', "\\"))
    } else {
        PathBuf::from(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct Wrapper {
        #[serde(with = "crate::unix_path")]
        path: PathBuf,
        #[serde(with = "crate::unix_path::map")]
        map: BTreeMap<PathBuf, u8>,
    }

    fn native(unix: &str) -> PathBuf {
        unix.split('/').collect()
    }

    #[test]
    fn written_with_forward_slashes_and_read_back_natively() {
        let w = Wrapper {
            path: native("working/sub/file.txt"),
            map: BTreeMap::from([(native("working/a/b"), 1)]),
        };
        let json = serde_json::to_string(&w).unwrap();
        assert_eq!(
            json,
            r#"{"path":"working/sub/file.txt","map":{"working/a/b":1}}"#
        );
        assert_eq!(serde_json::from_str::<Wrapper>(&json).unwrap(), w);
    }

    #[cfg(unix)]
    #[test]
    fn backslash_is_an_ordinary_character_on_unix() {
        let path = PathBuf::from(r"dir/odd\name");
        assert_eq!(to_unix(&path).unwrap(), r"dir/odd\name");
        assert_eq!(from_unix(r"dir/odd\name"), path);
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_path_fails_to_serialize() {
        use std::os::unix::ffi::OsStrExt;
        let path = Path::new(std::ffi::OsStr::from_bytes(b"bad\xff"));
        assert!(to_unix(path).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_round_trip_exactly() {
        for native in [
            r"working\a\b.txt",
            r"C:\Tools\x.exe",
            r"\\server\share\f",
            r".",
        ] {
            let stored = to_unix(Path::new(native)).unwrap();
            assert!(!stored.contains('\\'), "{stored}");
            assert_eq!(from_unix(&stored), Path::new(native));
        }
    }
}
