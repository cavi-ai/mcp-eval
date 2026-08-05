use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::Path;

use anyhow::Context;
use sha2::{Digest, Sha256};

use crate::errtemplate;

/// Per-install secret used to salt error fingerprints. Generated once and
/// persisted at `<root>/salt`; never leaves the machine and never appears in
/// stored output.
pub struct Salt([u8; 32]);

impl Salt {
    /// Loads the salt from `<root>/salt`, creating it (mode 0600 on Unix)
    /// from two random UUIDs if it does not yet exist.
    pub fn load(root: &Path) -> anyhow::Result<Self> {
        let path = root.join("salt");
        match fs::read(&path) {
            Ok(bytes) => {
                let bytes: [u8; 32] = bytes.try_into().map_err(|bytes: Vec<u8>| {
                    anyhow::anyhow!(
                        "salt file at {} has {} bytes, expected 32",
                        path.display(),
                        bytes.len()
                    )
                })?;
                Ok(Self(bytes))
            }
            Err(error) if error.kind() == ErrorKind::NotFound => Self::create(&path),
            Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// A fixed salt for tests. Never used outside the test suite.
    pub fn for_tests() -> Self {
        Self([0u8; 32])
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    fn bytes(&self) -> &[u8; 32] {
        &self.0
    }

    fn create(path: &Path) -> anyhow::Result<Self> {
        let salt = Self::generate();
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(path)
            .with_context(|| format!("creating {}", path.display()))?;
        file.write_all(&salt.0)
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(salt)
    }

    fn generate() -> Self {
        let mut bytes = [0u8; 32];
        bytes[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        bytes[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        Self(bytes)
    }
}

/// Lowercase hex fingerprint of the first 8 bytes of
/// `SHA256(salt || 0x00 || skeleton(message))`. Non-invertible: the message
/// never appears in the result, and a different salt yields a different
/// fingerprint for the same message.
pub fn template_id(salt: &Salt, message: &str) -> String {
    let skeleton = errtemplate::skeleton(message);
    let mut hasher = Sha256::new();
    hasher.update(salt.bytes());
    hasher.update([0u8]);
    hasher.update(skeleton.as_bytes());
    let digest = hasher.finalize();
    let mut id = String::with_capacity(16);
    for byte in &digest[..8] {
        id.push_str(&format!("{byte:02x}"));
    }
    id
}
