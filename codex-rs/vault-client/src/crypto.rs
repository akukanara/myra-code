//! The cryptography behind a Personal Memory vault, as the client side of it.
//!
//! MyraRouter stores this vault as ciphertext and holds no key that opens it. Every
//! primitive here therefore has an exact counterpart in the dashboard's
//! `src/shared/vault/keys.js` and `item.js`; the two must agree byte for byte or a
//! memory written in the browser cannot be read here. Where a constant or a label is
//! duplicated across the two languages it is called out, because a silent divergence
//! shows up as an authentication failure with no clue why.
use aes_gcm::Aes256Gcm;
use aes_gcm::Key;
use aes_gcm::KeyInit;
use aes_gcm::Nonce;
use aes_gcm::aead::Aead;
use aes_gcm::aead::Payload;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hkdf::Hkdf;
use p256::PublicKey;
use p256::SecretKey;
use p256::ecdh::diffie_hellman;
use p256::elliptic_curve::sec1::ToEncodedPoint;
use rand::RngCore;
use sha2::Digest;
use sha2::Sha256;
use zeroize::Zeroize;
use zeroize::Zeroizing;

/// Schema version shared with the dashboard. Appears in every AAD label, so bumping it
/// on one side alone makes all existing ciphertext unreadable on the other.
pub const SCHEMA_VERSION: u32 = 1;

const DEVICE_HKDF_INFO: &[u8] = b"myravault/device/v1";

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("value is not valid base64")]
    Base64,
    #[error("{field} has the wrong length: expected {expected} bytes, got {actual}")]
    Length {
        field: &'static str,
        expected: usize,
        actual: usize,
    },
    #[error("decryption failed -- wrong key, or the data was altered")]
    Decrypt,
    #[error("encryption failed")]
    Encrypt,
    #[error("key derivation failed")]
    Derive,
    #[error("the device public key is not a valid P-256 point")]
    PublicKey,
}

/// Decode base64, accepting both the url-safe unpadded form the dashboard emits and the
/// standard padded form, because JSON from other tools may use either.
pub fn decode_base64(value: &str) -> Result<Vec<u8>, CryptoError> {
    URL_SAFE_NO_PAD
        .decode(value.trim_end_matches('='))
        .or_else(|_| BASE64_STANDARD.decode(value))
        .map_err(|_| CryptoError::Base64)
}

/// Encode in the url-safe unpadded form, which is what the server validates against.
pub fn encode_base64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn expect_len(field: &'static str, bytes: &[u8], expected: usize) -> Result<(), CryptoError> {
    if bytes.len() == expected {
        Ok(())
    } else {
        Err(CryptoError::Length {
            field,
            expected,
            actual: bytes.len(),
        })
    }
}

/// A Vault Data Key: 32 bytes that decrypt everything in one vault.
///
/// Wrapped in `Zeroizing` rather than a bare array so the bytes are cleared when the
/// value is dropped. That does not defeat a determined memory dump, but it keeps a
/// long-lived process from leaving the key lying around after it is done with it.
#[derive(Clone)]
pub struct VaultKey(Zeroizing<[u8; 32]>);

impl VaultKey {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        expect_len("vault key", bytes, 32)?;
        let mut key = [0u8; 32];
        key.copy_from_slice(bytes);
        Ok(Self(Zeroizing::new(key)))
    }

    fn cipher(&self) -> Aes256Gcm {
        Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(self.0.as_ref()))
    }

    /// Decrypt with associated data. AES-GCM authenticates the AAD, which is how a
    /// ciphertext moved between items or vaults fails instead of decrypting into the
    /// wrong place.
    pub fn open(&self, nonce: &[u8], aad: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        expect_len("nonce", nonce, 12)?;
        self.cipher()
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad,
                },
            )
            .map_err(|_| CryptoError::Decrypt)
    }

    pub fn seal(
        &self,
        nonce: &[u8],
        aad: &[u8],
        plaintext: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        expect_len("nonce", nonce, 12)?;
        self.cipher()
            .encrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .map_err(|_| CryptoError::Encrypt)
    }
}

impl std::fmt::Debug for VaultKey {
    /// Never print the key. A `{vault_key:?}` in a log line would undo the whole design.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("VaultKey(<redacted>)")
    }
}

/// AAD for an item's content. Mirrors `itemAad` in the dashboard's `item.js`.
pub fn item_aad(vault_id: &str, item_id: &str, key_version: u32) -> Vec<u8> {
    format!("myravault/item/v{SCHEMA_VERSION}|{vault_id}|{item_id}|{key_version}").into_bytes()
}

/// AAD for an item's embedding vector. A separate label from the content, so a vector
/// blob cannot be passed off as content or the reverse.
pub fn vector_aad(vault_id: &str, item_id: &str, key_version: u32) -> Vec<u8> {
    format!("myravault/vec/v{SCHEMA_VERSION}|{vault_id}|{item_id}|{key_version}").into_bytes()
}

/// AAD for a wrapped key. Binds to the vault's random salt rather than its id, because
/// the browser wraps the key before the vault exists and has no id yet.
pub fn wrap_aad(aad_salt: &str) -> Vec<u8> {
    format!("myravault/wrap/v{SCHEMA_VERSION}|{aad_salt}").into_bytes()
}

/// Unwrap a device's copy of the vault key.
///
/// ECDH-ES: the dashboard generated a throwaway keypair, derived a shared secret with
/// this device's public key, and wrapped the vault key with it. Reproducing the secret
/// needs this device's private key and the ephemeral public key that came with the
/// envelope -- which is why deleting the envelope server-side cuts this device off even
/// though it still holds its own key.
pub fn unwrap_vault_key_for_device(
    device_secret: &SecretKey,
    ephemeral_public: &PublicKey,
    aad_salt: &str,
    nonce: &[u8],
    wrapped: &[u8],
) -> Result<VaultKey, CryptoError> {
    let wrapping_key = device_wrapping_key(device_secret, ephemeral_public, aad_salt)?;
    let plain = wrapping_key.open(nonce, &wrap_aad(aad_salt), wrapped)?;
    let key = VaultKey::from_bytes(&plain);
    let mut plain = plain;
    plain.zeroize();
    key
}

/// The wrapping side of the same derivation, exposed for tests so the round trip can be
/// exercised without a browser: the dashboard derives from (ephemeral private, device
/// public) while the device derives from (device private, ephemeral public).
#[cfg(test)]
pub fn device_wrapping_key_for_test(
    ephemeral_secret: &SecretKey,
    device_public: &PublicKey,
    aad_salt: &str,
) -> Result<VaultKey, CryptoError> {
    device_wrapping_key(ephemeral_secret, device_public, aad_salt)
}

fn device_wrapping_key(
    device_secret: &SecretKey,
    ephemeral_public: &PublicKey,
    aad_salt: &str,
) -> Result<VaultKey, CryptoError> {
    let shared = diffie_hellman(device_secret.to_nonzero_scalar(), ephemeral_public.as_affine());
    let salt = decode_base64(aad_salt)?;
    let mut derived = Zeroizing::new([0u8; 32]);
    // HKDF over the raw shared secret, matching hkdfWrappingKey in the dashboard's
    // keys.js: salt = the vault's AAD salt, info = the device label.
    Hkdf::<Sha256>::new(Some(&salt), shared.raw_secret_bytes())
        .expand(DEVICE_HKDF_INFO, derived.as_mut())
        .map_err(|_| CryptoError::Derive)?;
    VaultKey::from_bytes(derived.as_ref())
}

/// A P-256 public key from the JWK coordinates the server stores.
pub fn public_key_from_jwk(x: &str, y: &str) -> Result<PublicKey, CryptoError> {
    let x = decode_base64(x)?;
    let y = decode_base64(y)?;
    expect_len("jwk.x", &x, 32)?;
    expect_len("jwk.y", &y, 32)?;
    let mut sec1 = Vec::with_capacity(65);
    sec1.push(0x04);
    sec1.extend_from_slice(&x);
    sec1.extend_from_slice(&y);
    PublicKey::from_sec1_bytes(&sec1).map_err(|_| CryptoError::PublicKey)
}

/// The JWK coordinates for a public key, in the shape the server accepts. Only x and y
/// travel: the server rejects a JWK carrying private key material.
pub fn jwk_coordinates(public: &PublicKey) -> (String, String) {
    let point = public.as_affine().to_encoded_point(false);
    let x = point.x().map(|x| encode_base64(&x)).unwrap_or_default();
    let y = point.y().map(|y| encode_base64(&y)).unwrap_or_default();
    (x, y)
}

/// Short fingerprint of a public key, matching `fingerprintJwk` in
/// `src/lib/memory/pairing.js`. Shown next to the pairing code so a user can compare the
/// key itself, not only the code.
pub fn jwk_fingerprint(x: &str, y: &str) -> String {
    let canonical = format!(r#"{{"crv":"P-256","kty":"EC","x":"{x}","y":"{y}"}}"#);
    let digest = Sha256::digest(canonical.as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    hex[..16]
        .as_bytes()
        .chunks(4)
        .map(|chunk| String::from_utf8_lossy(chunk).to_uppercase())
        .collect::<Vec<_>>()
        .join("-")
}

pub fn random_nonce() -> [u8; 12] {
    let mut nonce = [0u8; 12];
    rand::rng().fill_bytes(&mut nonce);
    nonce
}

pub fn random_bytes(length: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; length];
    rand::rng().fill_bytes(&mut bytes);
    bytes
}

/// Generate a P-256 private key from `rand` bytes.
///
/// Deliberately not `SecretKey::random`: p256 0.13 takes an RNG from rand_core 0.6 while
/// this workspace is on rand 0.9, whose types do not implement that trait. Rejection
/// sampling over raw bytes sidesteps two incompatible RNG traits entirely -- and is what
/// `from_slice` already validates for, since not every 32-byte string is a valid scalar.
pub fn generate_secret_key() -> Result<SecretKey, CryptoError> {
    for _ in 0..64 {
        let candidate = random_bytes(32);
        if let Ok(secret) = SecretKey::from_slice(&candidate) {
            return Ok(secret);
        }
    }
    // 64 consecutive rejections is not something a working RNG does; treating it as an
    // error beats looping forever.
    Err(CryptoError::Derive)
}
