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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Cursor};

    // Known-answer vectors from FIPS 180-2 / NIST.
    const SHA256_EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    const SHA256_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    const SHA256_MILLION_A: &str =
        "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0";
    const SHA512_EMPTY: &str = "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce\
                                47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e";
    const SHA512_ABC: &str = "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
                              2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f";

    fn digest(alg: HashAlgorithm, data: &[u8]) -> String {
        alg.digest(Cursor::new(data)).unwrap()
    }

    /// Hex of a one-shot hash of the whole input, as a reference to compare
    /// the streaming implementation against.
    fn reference<D: Digest>(data: &[u8]) -> String {
        D::digest(data).iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Deterministic, non-repeating-ish test data, so a chunk that is hashed
    /// twice or skipped changes the result.
    fn sample_data(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 31 + i / 251) as u8).collect()
    }

    /// Yields at most `max` bytes per `read` call.
    struct ShortReads<R> {
        inner: R,
        max: usize,
    }

    impl<R: Read> Read for ShortReads<R> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = buf.len().min(self.max);
            self.inner.read(&mut buf[..n])
        }
    }

    /// Fails with `Interrupted` on every other `read` call, starting with the first.
    struct Interrupting<R> {
        inner: R,
        interrupt_next: bool,
    }

    impl<R: Read> Read for Interrupting<R> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.interrupt_next = !self.interrupt_next;
            if !self.interrupt_next {
                return Err(io::Error::from(ErrorKind::Interrupted));
            }
            self.inner.read(buf)
        }
    }

    /// Returns some data, then a hard error.
    struct FailsAfter {
        remaining: usize,
    }

    impl Read for FailsAfter {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.remaining == 0 {
                return Err(io::Error::other("disk on fire"));
            }
            let n = buf.len().min(self.remaining);
            buf[..n].fill(0);
            self.remaining -= n;
            Ok(n)
        }
    }

    #[test]
    fn sha256_known_vectors() {
        assert_eq!(digest(HashAlgorithm::Sha256, b""), SHA256_EMPTY);
        assert_eq!(digest(HashAlgorithm::Sha256, b"abc"), SHA256_ABC);
    }

    #[test]
    fn sha512_known_vectors() {
        assert_eq!(digest(HashAlgorithm::Sha512, b""), SHA512_EMPTY);
        assert_eq!(digest(HashAlgorithm::Sha512, b"abc"), SHA512_ABC);
    }

    #[test]
    fn million_a_spans_many_chunks() {
        // 1,000,000 bytes = 15 full 64 KiB chunks plus a partial one, so a
        // bug that hashes the whole buffer instead of `&buf[..n]` shows here.
        let data = vec![b'a'; 1_000_000];
        assert_eq!(digest(HashAlgorithm::Sha256, &data), SHA256_MILLION_A);
    }

    #[test]
    fn multi_chunk_input_matches_one_shot_hash() {
        // ~200 KB: three full chunks plus a partial one.
        let data = sample_data(200_000);
        assert_eq!(
            digest(HashAlgorithm::Sha256, &data),
            reference::<Sha256>(&data)
        );
        assert_eq!(
            digest(HashAlgorithm::Sha512, &data),
            reference::<Sha512>(&data)
        );
    }

    #[test]
    fn chunk_boundaries_are_handled_exactly() {
        const CHUNK: usize = 64 * 1024;
        for len in [CHUNK - 1, CHUNK, CHUNK + 1, 2 * CHUNK, 2 * CHUNK + 1] {
            let data = sample_data(len);
            assert_eq!(
                digest(HashAlgorithm::Sha256, &data),
                reference::<Sha256>(&data),
                "length {len}"
            );
        }
    }

    #[test]
    fn short_reads_give_the_same_digest() {
        let data = sample_data(200_000);
        for max in [1, 7, 4096, 65_535] {
            let reader = ShortReads {
                inner: Cursor::new(&data),
                max,
            };
            assert_eq!(
                HashAlgorithm::Sha256.digest(reader).unwrap(),
                reference::<Sha256>(&data),
                "max {max} bytes per read"
            );
        }
    }

    #[test]
    fn interrupted_reads_are_retried() {
        let data = sample_data(200_000);
        let reader = Interrupting {
            inner: ShortReads {
                inner: Cursor::new(&data),
                max: 10_000,
            },
            interrupt_next: false,
        };
        assert_eq!(
            HashAlgorithm::Sha256.digest(reader).unwrap(),
            reference::<Sha256>(&data)
        );
    }

    #[test]
    fn other_read_errors_are_propagated() {
        let err = HashAlgorithm::Sha256
            .digest(FailsAfter { remaining: 100_000 })
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Other);
    }

    #[test]
    fn digest_is_lowercase_hex_of_the_right_length() {
        for (alg, len) in [(HashAlgorithm::Sha256, 64), (HashAlgorithm::Sha512, 128)] {
            let hex = digest(alg, b"abc");
            assert_eq!(hex.len(), len, "{alg:?}");
            assert!(
                hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
                "{alg:?}: {hex}"
            );
        }
    }

    #[test]
    fn prefixed_digest_adds_algorithm_name() {
        assert_eq!(
            HashAlgorithm::Sha256
                .prefixed_digest(Cursor::new(b"abc"))
                .unwrap(),
            format!("sha256:{SHA256_ABC}")
        );
        assert_eq!(
            HashAlgorithm::Sha512
                .prefixed_digest(Cursor::new(b"abc"))
                .unwrap(),
            format!("sha512:{SHA512_ABC}")
        );
    }

    #[test]
    fn name_round_trips_through_from_str() {
        for alg in HashAlgorithm::ALL {
            assert_eq!(alg.name().parse::<HashAlgorithm>().unwrap(), alg);
        }
    }

    #[test]
    fn name_matches_serde_name() {
        // The journal prefix and the config value must stay the same string.
        for alg in HashAlgorithm::ALL {
            assert_eq!(
                serde_json::to_string(&alg).unwrap(),
                format!("\"{}\"", alg.name())
            );
        }
    }

    #[test]
    fn from_str_rejects_unknown_and_prefixed_names() {
        for bad in ["md5", "SHA256", "sha-256", "", "sha256:abc"] {
            assert!(
                matches!(
                    bad.parse::<HashAlgorithm>(),
                    Err(Error::UnsupportedHashAlgorithm(ref s)) if s == bad
                ),
                "{bad:?} should be rejected"
            );
        }
    }
}
