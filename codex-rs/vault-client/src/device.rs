//! This machine's identity to a Personal Memory vault.
//!
//! MyraCode holds a P-256 private key and nothing else. The vault key it opens lives on
//! the server, wrapped to the matching public key, which is what makes revocation real:
//! the user deletes that wrapped copy and this key becomes useless, even though it is
//! still sitting on disk.
//!
//! The private key must therefore be treated as a credential but NOT as the vault key.
//! Two rules follow, and both are load-bearing:
//!
//!   * the file is created 0600, and a file that is not 0600 is refused rather than
//!     quietly used;
//!   * the unwrapped vault key is never written anywhere. It exists in memory for the
//!     length of a session and is re-fetched next time. Caching it on disk would make
//!     revocation cosmetic.
use std::path::Path;
use std::path::PathBuf;

use p256::SecretKey;
use serde::Deserialize;
use serde::Serialize;

use crate::crypto;

#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error("could not read the device key at {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not write the device key to {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the device key at {path} is not valid JSON: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("the device key at {path} is readable by other users (mode {mode:o}); fix it with chmod 600")]
    Permissions { path: PathBuf, mode: u32 },
    #[error("the device key at {path} is malformed")]
    Malformed { path: PathBuf },
}

/// What is persisted. `secret` is the private scalar; everything else is derived from it
/// and stored only so a human can recognise the file.
#[derive(Debug, Serialize, Deserialize)]
struct StoredDeviceKey {
    version: u32,
    secret: String,
    public_x: String,
    public_y: String,
    fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    device_id: Option<String>,
}

/// This machine's key, plus the identifiers the server needs to recognise it.
pub struct DeviceIdentity {
    path: PathBuf,
    secret: SecretKey,
    public_x: String,
    public_y: String,
    fingerprint: String,
    device_id: Option<String>,
}

impl DeviceIdentity {
    /// Load the key for a vault, creating one on first use.
    ///
    /// Keyed per vault: a key enrolled against one vault is not silently an identity for
    /// another, so approving MyraCode for a work vault does not hand it a personal one.
    pub async fn load_or_create(root: &Path, vault_id: &str) -> Result<Self, DeviceError> {
        let path = key_path(root, vault_id);
        match tokio::fs::read(&path).await {
            Ok(bytes) => Self::from_stored(path, &bytes).await,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Self::create(path).await
            }
            Err(source) => Err(DeviceError::Read { path, source }),
        }
    }

    async fn from_stored(path: PathBuf, bytes: &[u8]) -> Result<Self, DeviceError> {
        check_permissions(&path).await?;
        let stored: StoredDeviceKey = serde_json::from_slice(bytes).map_err(|source| {
            DeviceError::Parse {
                path: path.clone(),
                source,
            }
        })?;
        let secret_bytes = crypto::decode_base64(&stored.secret)
            .map_err(|_| DeviceError::Malformed { path: path.clone() })?;
        let secret = SecretKey::from_slice(&secret_bytes)
            .map_err(|_| DeviceError::Malformed { path: path.clone() })?;
        Ok(Self {
            path,
            secret,
            public_x: stored.public_x,
            public_y: stored.public_y,
            fingerprint: stored.fingerprint,
            device_id: stored.device_id,
        })
    }

    async fn create(path: PathBuf) -> Result<Self, DeviceError> {
        let secret = crypto::generate_secret_key().map_err(|_| DeviceError::Malformed {
            path: path.clone(),
        })?;
        let (public_x, public_y) = crypto::jwk_coordinates(&secret.public_key());
        let fingerprint = crypto::jwk_fingerprint(&public_x, &public_y);
        let identity = Self {
            path,
            secret,
            public_x,
            public_y,
            fingerprint,
            device_id: None,
        };
        identity.persist().await?;
        Ok(identity)
    }

    async fn persist(&self) -> Result<(), DeviceError> {
        let stored = StoredDeviceKey {
            version: 1,
            secret: crypto::encode_base64(self.secret.to_bytes().as_slice()),
            public_x: self.public_x.clone(),
            public_y: self.public_y.clone(),
            fingerprint: self.fingerprint.clone(),
            device_id: self.device_id.clone(),
        };
        let json = serde_json::to_vec_pretty(&stored).map_err(|source| DeviceError::Parse {
            path: self.path.clone(),
            source,
        })?;

        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|source| DeviceError::Write {
                    path: parent.to_path_buf(),
                    source,
                })?;
        }
        write_private(&self.path, &json).await
    }

    /// Record the device id the server assigned at enrolment, so later runs can ask for
    /// their own wrapped key without enrolling again.
    pub async fn set_device_id(&mut self, device_id: String) -> Result<(), DeviceError> {
        self.device_id = Some(device_id);
        self.persist().await
    }

    pub fn device_id(&self) -> Option<&str> {
        self.device_id.as_deref()
    }

    pub fn secret(&self) -> &SecretKey {
        &self.secret
    }

    pub fn public_jwk(&self) -> (&str, &str) {
        (&self.public_x, &self.public_y)
    }

    /// Shown beside the pairing code, so the user can compare the key as well as the code.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn key_path(root: &Path, vault_id: &str) -> PathBuf {
    // Vault ids are server-generated UUIDs, but a path is a path: keep only characters
    // that cannot escape the directory.
    let safe: String = vault_id
        .chars()
        .map(|character| if character.is_ascii_alphanumeric() { character } else { '-' })
        .collect();
    root.join("vault").join(format!("device-{safe}.json"))
}

#[cfg(unix)]
async fn write_private(path: &Path, contents: &[u8]) -> Result<(), DeviceError> {
    use std::os::unix::fs::OpenOptionsExt;

    // Created 0600 from the outset rather than written and then chmodded -- the gap
    // between those two is a window where the key is world-readable.
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true).mode(0o600);
    let mut file = options
        .open(path)
        .await
        .map_err(|source| DeviceError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    use tokio::io::AsyncWriteExt;
    file.write_all(contents)
        .await
        .map_err(|source| DeviceError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    file.flush().await.map_err(|source| DeviceError::Write {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(not(unix))]
async fn write_private(path: &Path, contents: &[u8]) -> Result<(), DeviceError> {
    // Windows has no mode bits; the file inherits the user profile's ACL, which is the
    // same protection the rest of the credential store relies on.
    tokio::fs::write(path, contents)
        .await
        .map_err(|source| DeviceError::Write {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(unix)]
async fn check_permissions(path: &Path) -> Result<(), DeviceError> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|source| DeviceError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    let mode = metadata.permissions().mode() & 0o777;
    // Refused rather than quietly used: a key another account can read is a key another
    // account can use, and using it anyway would hide that from the person who owns it.
    if mode & 0o077 != 0 {
        return Err(DeviceError::Permissions {
            path: path.to_path_buf(),
            mode,
        });
    }
    Ok(())
}

#[cfg(not(unix))]
async fn check_permissions(_path: &Path) -> Result<(), DeviceError> {
    Ok(())
}
