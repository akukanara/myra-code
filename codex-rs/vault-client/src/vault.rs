//! An open Personal Memory vault: the key in memory, the items decrypted, searchable.
//!
//! The division of labour with MyraRouter is the whole point. The server keeps ciphertext
//! and cannot read it; everything legible happens here. So this module holds the decrypted
//! index, and the server is never asked to filter or rank anything.
use serde::Deserialize;
use serde::Serialize;

use crate::client::ItemRow;
use crate::client::ItemUpload;
use crate::client::VaultApi;
use crate::client::VaultSummary;
use crate::crypto;
use crate::crypto::VaultKey;
use crate::device::DeviceIdentity;

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error(transparent)]
    Api(#[from] crate::client::ApiError),
    #[error(transparent)]
    Crypto(#[from] crypto::CryptoError),
    #[error(transparent)]
    Device(#[from] crate::device::DeviceError),
    #[error("this device is not approved for the vault yet")]
    AwaitingApproval {
        pairing_code: String,
        fingerprint: String,
        vault_name: String,
    },
    #[error("this device's access was revoked; enrol it again to regain access")]
    Revoked,
    #[error("no vault is available on this account -- create one in the dashboard first")]
    NoVault,
    #[error("the wrapped key is missing its ephemeral public key")]
    MalformedEnvelope,
    #[error("a memory could not be stored: {0}")]
    WriteRejected(String),
}

/// One memory, as it is written and read.
///
/// Field names match the dashboard's `makeItemPayload` exactly. A rename on one side makes
/// the other side's memories arrive with empty titles, silently.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryPayload {
    #[serde(rename = "v")]
    pub version: u32,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

impl MemoryPayload {
    pub fn new(title: impl Into<String>, body: impl Into<String>, tags: Vec<String>, now: String) -> Self {
        Self {
            version: crypto::SCHEMA_VERSION,
            title: title.into(),
            body: body.into(),
            tags,
            project: None,
            // Recorded so the dashboard can show which memories the agent wrote.
            source: "myracode".to_string(),
            pinned: false,
            created_at: now.clone(),
            updated_at: now,
        }
    }

    /// The text an embedding is computed over. Title and tags are included because a
    /// memory titled "postgres decision" should be findable by that phrase even when the
    /// body never repeats it -- matching `indexableText` in the dashboard's sync.js.
    pub fn indexable_text(&self) -> String {
        let mut parts = Vec::new();
        if !self.title.is_empty() {
            parts.push(self.title.clone());
        }
        if !self.tags.is_empty() {
            parts.push(self.tags.join(" "));
        }
        if !self.body.is_empty() {
            parts.push(self.body.clone());
        }
        parts.join("\n")
    }
}

/// A decrypted memory plus the metadata needed to write it back.
#[derive(Debug, Clone)]
pub struct MemoryEntry {
    pub id: String,
    pub rev: u32,
    pub seq: u64,
    pub key_version: u32,
    pub payload: MemoryPayload,
    /// int8, unit-scaled by the writer. Absent for a vault with no vector index, or for a
    /// memory saved before one existed.
    pub vector: Option<Vec<i8>>,
}

/// The immutable half of an open vault: credentials, key, and the calls that use them.
///
/// Split from the index on purpose. Every await lives here and needs only `&self`, so a caller
/// sharing the index behind a lock never has to hold that lock across a network call --
/// which the workspace lints forbid outright, and which would serialise the whole tool
/// surface behind one HTTP request even if they did not.
#[derive(Clone)]
pub struct VaultSession {
    api: VaultApi,
    vault: VaultSummary,
    device_id: String,
    key: VaultKey,
}

/// The mutable half: decrypted memories and how far the sync has read.
#[derive(Default)]
pub struct VaultIndex {
    entries: Vec<MemoryEntry>,
    cursor: u64,
}

impl VaultSession {
    /// Open a vault for this machine.
    ///
    /// Enrols on first use and returns `AwaitingApproval` carrying the pairing code, so the
    /// caller can tell the user exactly what to type. That is not an error to retry blindly:
    /// nothing changes until a human approves it in the dashboard.
    pub async fn open(
        api: VaultApi,
        identity: &mut DeviceIdentity,
        vault_id: Option<&str>,
        label: &str,
    ) -> Result<Self, VaultError> {
        let (vault, device_id) = Self::resolve(&api, identity, vault_id, label).await?;

        let envelope = match api.wrapped_key(&vault.id, &device_id).await {
            Ok(envelope) => envelope,
            Err(error) if error.is_revoked() => return Err(VaultError::Revoked),
            Err(error) if error.is_pending_approval() => {
                // Re-enrol so the user gets a fresh, unexpired code rather than being told to
                // approve something they can no longer see.
                let enrolled = Self::enroll(&api, identity, Some(&vault.id), label).await?;
                return Err(VaultError::AwaitingApproval {
                    pairing_code: enrolled.pairing_code,
                    fingerprint: enrolled.fingerprint,
                    vault_name: enrolled.vault_name,
                });
            }
            Err(error) => return Err(error.into()),
        };

        let jwk = envelope
            .ephemeral_pub_jwk
            .as_ref()
            .ok_or(VaultError::MalformedEnvelope)?;
        let ephemeral = crypto::public_key_from_jwk(&jwk.x, &jwk.y)?;
        // The AAD salt the dashboard wrapped with, recorded on the key row and returned
        // alongside the envelope. Not a secret -- it is the value both sides must agree on for
        // the AAD to match.
        let aad_salt = envelope.aad_salt.clone().unwrap_or_default();
        let key = crypto::unwrap_vault_key_for_device(
            identity.secret(),
            &ephemeral,
            &aad_salt,
            &crypto::decode_base64(&envelope.wrap_nonce)?,
            &crypto::decode_base64(&envelope.wrapped_key)?,
        )?;

        Ok(Self {
            api,
            vault,
            device_id,
            key,
        })
    }

    async fn resolve(
        api: &VaultApi,
        identity: &mut DeviceIdentity,
        vault_id: Option<&str>,
        label: &str,
    ) -> Result<(VaultSummary, String), VaultError> {
        if let Some(device_id) = identity.device_id() {
            let device_id = device_id.to_string();
            let vaults = api.vaults(Some(&device_id)).await?;
            if let Some(vault) = pick_vault(&vaults, vault_id) {
                return Ok((vault, device_id));
            }
        }

        let enrolled = Self::enroll(api, identity, vault_id, label).await?;
        Err(VaultError::AwaitingApproval {
            pairing_code: enrolled.pairing_code,
            fingerprint: enrolled.fingerprint,
            vault_name: enrolled.vault_name,
        })
    }

    async fn enroll(
        api: &VaultApi,
        identity: &mut DeviceIdentity,
        vault_id: Option<&str>,
        label: &str,
    ) -> Result<crate::client::EnrollResponse, VaultError> {
        let (x, y) = identity.public_jwk();
        let jwk = crate::client::PublicJwk {
            kty: "EC".to_string(),
            crv: "P-256".to_string(),
            x: x.to_string(),
            y: y.to_string(),
        };
        let enrolled = api.enroll(vault_id, label, &jwk).await.map_err(|error| {
            if error.code() == Some("no_vault") {
                VaultError::NoVault
            } else {
                VaultError::Api(error)
            }
        })?;
        identity.set_device_id(enrolled.device_id.clone()).await?;
        Ok(enrolled)
    }

    pub fn vault(&self) -> &VaultSummary {
        &self.vault
    }

    /// Fetch one page of ciphertext. Needs no lock and mutates nothing.
    pub async fn fetch_since(&self, since: u64) -> Result<crate::client::ItemPage, VaultError> {
        Ok(self.api.pull(&self.vault.id, &self.device_id, since).await?)
    }

    /// Encrypt and store one memory, returning the entry to add to an index.
    ///
    /// `vector` is supplied by the caller: computing one means sending the text to an embedding
    /// provider, which is a decision for the layer that knows the vault's mode.
    pub async fn write(
        &self,
        payload: MemoryPayload,
        vector: Option<Vec<i8>>,
    ) -> Result<MemoryEntry, VaultError> {
        let id = new_item_id();
        let aad = crypto::item_aad(&self.vault.id, &id, self.vault.key_version);
        let nonce = crypto::random_nonce();
        let plaintext = serde_json::to_vec(&payload)
            .map_err(|error| VaultError::WriteRejected(error.to_string()))?;
        let cipher = self.key.seal(&nonce, &aad, &plaintext)?;

        let sealed_vector = match vector.as_ref() {
            Some(vector) => {
                let vector_aad = crypto::vector_aad(&self.vault.id, &id, self.vault.key_version);
                let vector_nonce = crypto::random_nonce();
                let bytes: Vec<u8> = vector.iter().map(|value| *value as u8).collect();
                Some((
                    crypto::encode_base64(&self.key.seal(&vector_nonce, &vector_aad, &bytes)?),
                    crypto::encode_base64(&vector_nonce),
                ))
            }
            None => None,
        };

        let upload = ItemUpload {
            id: id.clone(),
            cipher: crypto::encode_base64(&cipher),
            nonce: crypto::encode_base64(&nonce),
            vec: sealed_vector.as_ref().map(|(vec, _)| vec.clone()),
            vec_nonce: sealed_vector.as_ref().map(|(_, nonce)| nonce.clone()),
            rev: None,
        };

        let response = self
            .api
            .push(&self.vault.id, &self.device_id, vec![upload])
            .await?;
        let outcome = response
            .results
            .into_iter()
            .next()
            .ok_or_else(|| VaultError::WriteRejected("no outcome returned".to_string()))?;
        if !outcome.ok {
            return Err(VaultError::WriteRejected(
                outcome.code.unwrap_or_else(|| "rejected".to_string()),
            ));
        }

        Ok(MemoryEntry {
            id,
            rev: outcome.rev.unwrap_or(1),
            seq: outcome.seq.unwrap_or(0),
            key_version: self.vault.key_version,
            payload,
            vector,
        })
    }

    /// Decrypt a row.
    ///
    /// Returns `None` rather than an error for a row that will not open: one corrupt or tampered
    /// row must not make the whole vault unreadable, and a row stamped with a key version this
    /// session does not hold is the ordinary state of a rotation in progress.
    pub fn decrypt_row(&self, row: &ItemRow) -> Option<MemoryEntry> {
        let cipher = row.cipher.as_ref()?;
        let nonce = row.nonce.as_ref()?;
        let aad = crypto::item_aad(&self.vault.id, &row.id, row.key_version);
        let plain = self
            .key
            .open(
                &crypto::decode_base64(nonce).ok()?,
                &aad,
                &crypto::decode_base64(cipher).ok()?,
            )
            .ok()?;
        let payload: MemoryPayload = serde_json::from_slice(&plain).ok()?;

        // Content readable but vector not: keep the memory and let it fall back to text search
        // rather than dropping it.
        let vector = match (row.vec.as_ref(), row.vec_nonce.as_ref()) {
            (Some(vec), Some(vec_nonce)) => {
                let vector_aad = crypto::vector_aad(&self.vault.id, &row.id, row.key_version);
                crypto::decode_base64(vec_nonce)
                    .ok()
                    .zip(crypto::decode_base64(vec).ok())
                    .and_then(|(nonce, cipher)| self.key.open(&nonce, &vector_aad, &cipher).ok())
                    .map(|bytes| bytes.into_iter().map(|byte| byte as i8).collect())
            }
            _ => None,
        };

        Some(MemoryEntry {
            id: row.id.clone(),
            rev: row.rev,
            seq: row.seq,
            key_version: row.key_version,
            payload,
            vector,
        })
    }
}

impl VaultIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    pub fn entries(&self) -> &[MemoryEntry] {
        &self.entries
    }

    pub fn get(&self, id: &str) -> Option<&MemoryEntry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    /// Merge a decrypted page. Synchronous by design: no lock is ever held across an await.
    pub fn apply(&mut self, cursor: u64, decrypted: Vec<Option<MemoryEntry>>, tombstones: Vec<String>) {
        self.cursor = self.cursor.max(cursor);
        if !tombstones.is_empty() {
            self.entries
                .retain(|entry| !tombstones.contains(&entry.id));
        }
        for entry in decrypted.into_iter().flatten() {
            match self.entries.iter_mut().find(|existing| existing.id == entry.id) {
                Some(existing) => *existing = entry,
                None => self.entries.push(entry),
            }
        }
        self.sort();
    }

    pub fn insert(&mut self, entry: MemoryEntry) {
        self.cursor = self.cursor.max(entry.seq);
        self.entries.retain(|existing| existing.id != entry.id);
        self.entries.push(entry);
        self.sort();
    }

    fn sort(&mut self) {
        self.entries.sort_by(|left, right| {
            right
                .payload
                .pinned
                .cmp(&left.payload.pinned)
                .then_with(|| right.payload.updated_at.cmp(&left.payload.updated_at))
        });
    }
}

fn pick_vault(vaults: &[VaultSummary], wanted: Option<&str>) -> Option<VaultSummary> {
    let candidate = match wanted {
        Some(id) => vaults.iter().find(|vault| vault.id == id),
        // "auto": the account's default, which the dashboard guarantees exists as soon as
        // there is any vault at all.
        None => vaults
            .iter()
            .find(|vault| vault.is_default)
            .or_else(|| vaults.first()),
    }?;
    // Enrolled but not yet approved is not usable; the caller re-enrols and reports the code.
    if candidate.has_key_for_this_device {
        Some(candidate.clone())
    } else {
        None
    }
}

fn new_item_id() -> String {
    let bytes = crypto::random_bytes(16);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
