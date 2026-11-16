//! Hash algorithms and streaming hashing.

use crate::error::Error;
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};
use std::io::{ErrorKind, Read};
use std::str::FromStr;

/// A supported hash algorithm. Selected per case with `dflog init --hash` and
/// stored in the config.
// The variants deliberately have no `///` comments: clap's `ValueEnum` would
// show them in `--help` and change the format of the possible-values list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum HashAlgorithm {
    #[default]
    Sha256,
    Sha512,
}

impl HashAlgorithm {
    /// Every supported algorithm. Used by [`FromStr`]; add new algorithms here
    /// as well as in [`HashAlgorithm::name`].
    const ALL: [Self; 2] = [HashAlgorithm::Sha256, HashAlgorithm::Sha512];

    /// Name used as the hash prefix in the journal (`"sha256"`). Matches the
    /// serde names used in the config.
    pub fn name(self) -> &'static str {
        match self {
            HashAlgorithm::Sha256 => "sha256",
            HashAlgorithm::Sha512 => "sha512",
        }
    }

    /// Hashes everything `reader` yields and returns the digest as lowercase
    /// hex, without a prefix. Reads in chunks, so large files are never loaded
    /// into memory.
    pub fn digest<R: Read>(self, reader: R) -> std::io::Result<String> {
        match self {
            HashAlgorithm::Sha256 => hash_with::<Sha256, R>(reader),
            HashAlgorithm::Sha512 => hash_with::<Sha512, R>(reader),
        }
    }

    /// Like [`HashAlgorithm::digest`], but prefixed with the algorithm name
    /// (`"sha256:..."`). This is the only place the prefix is added; every hash
    /// stored in the journal goes through here.
    pub fn prefixed_digest<R: Read>(self, reader: R) -> std::io::Result<String> {
        Ok(format!("{}:{}", self.name(), self.digest(reader)?))
    }
}

/// Parses an algorithm name exactly as [`HashAlgorithm::name`] returns it
/// (`"sha256"`, not `"sha256:..."`). Unknown names fail with
/// [`Error::UnsupportedHashAlgorithm`].
impl FromStr for HashAlgorithm {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        HashAlgorithm::ALL
            .into_iter()
            .find(|alg| alg.name() == s)
            .ok_or_else(|| Error::UnsupportedHashAlgorithm(s.to_string()))
    }
}

/// Streams `reader` through the hasher `D` in 64 KiB chunks and returns the
/// digest as lowercase hex.
///
/// Only the bytes actually read are hashed (`&buf[..n]`), and reads that fail
/// with `ErrorKind::Interrupted` are retried rather than treated as errors.
fn hash_with<D: Digest, R: Read>(mut reader: R) -> std::io::Result<String> {
    let mut algo = D::new();
    let mut buf = vec![0u8; 64 * 1024];

    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        algo.update(&buf[..n]);
    }

    Ok(algo.finalize().iter().map(|b| format!("{b:02x}")).collect())
}
