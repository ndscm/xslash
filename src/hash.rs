//! Content hashing and verification.
//!
//! blake3 is the only supported algorithm for now, but the [`Hash`](enum@Hash)
//! enum keeps the door open for others.

use ::anyhow::Context;
use ::anyhow::Result;
use ::anyhow::bail;
use ::serde::Deserialize;
use ::serde::Serialize;
use ::std::fs::File;
use ::std::io::Read;
use ::std::path::Path;

/// A supported content-hash algorithm.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Hash {
    /// Placeholder for an artifact whose algorithm isn't known yet, e.g. one
    /// being staged before its digest is computed.
    Unknown,

    /// The BLAKE3 hash, the only algorithm xslash currently computes.
    Blake3,
}

impl Hash {
    /// The on-disk / on-wire name of the algorithm (also the cache subdir).
    pub fn as_str(self) -> &'static str {
        match self {
            Hash::Unknown => "unknown",
            Hash::Blake3 => "blake3",
        }
    }

    /// Hash every byte produced by `reader`, returning the lowercase hex digest.
    pub fn hash_reader<R: Read>(self, mut reader: R) -> Result<String> {
        match self {
            Hash::Unknown => bail!("unknown hash algorithm"),
            Hash::Blake3 => {
                let mut hasher = blake3::Hasher::new();
                let mut buf = [0u8; 64 * 1024];
                loop {
                    let n = reader.read(&mut buf).context("reading input for hashing")?;
                    if n == 0 {
                        break;
                    }
                    hasher.update(&buf[..n]);
                }
                Ok(hasher.finalize().to_hex().to_string())
            }
        }
    }

    /// Hash an in-memory buffer, returning the lowercase hex digest.
    pub fn hash_bytes(self, bytes: &[u8]) -> Result<String> {
        match self {
            Hash::Unknown => bail!("unknown hash algorithm"),
            Hash::Blake3 => Ok(blake3::hash(bytes).to_hex().to_string()),
        }
    }

    /// Hash the contents of a file, returning the lowercase hex digest.
    pub fn hash_file(self, path: &Path) -> Result<String> {
        let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
        self.hash_reader(file)
    }

    /// Verify that `path` matches the expected `size` (when given) and `digest`.
    pub fn verify_file(self, path: &Path, digest: &str, size: Option<u64>) -> Result<()> {
        if let Some(expected) = size {
            let actual = ::std::fs::metadata(path)
                .with_context(|| format!("stat {}", path.display()))?
                .len();
            if actual != expected {
                bail!(
                    "size mismatch for {}: expected {expected}, got {actual}",
                    path.display()
                );
            }
        }

        let actual = self.hash_file(path)?;
        if !actual.eq_ignore_ascii_case(digest) {
            bail!(
                "{self} digest mismatch for {}: expected {digest}, got {actual}",
                path.display()
            );
        }

        Ok(())
    }
}

impl ::std::fmt::Display for Hash {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.write_str(self.as_str())
    }
}
