use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};
use std::io::{ErrorKind, Read};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum HashAlgorithm {
    #[default]
    Sha256,
    Sha512,
}

impl HashAlgorithm {
    pub fn digest<R: Read>(self, reader: R) -> std::io::Result<String> {
        match self {
            HashAlgorithm::Sha256 => hash_with::<Sha256, R>(reader),
            HashAlgorithm::Sha512 => hash_with::<Sha512, R>(reader),
        }
    }
}

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

    Ok(algo
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
