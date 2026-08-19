//! Client for a MyraRouter Personal Memory vault.
//!
//! A vault is encrypted on the client and stored as ciphertext MyraRouter cannot read. This
//! crate is the other half of that arrangement for MyraCode: it holds this machine's device
//! key, unwraps the vault key that was wrapped for it, decrypts the memories, and searches
//! them locally.
//!
//! Three properties are worth stating because they constrain everything else:
//!
//! 1. **The vault key is never written to disk.** It is fetched wrapped, unwrapped in
//!    memory, and dropped. That is what makes revocation real -- the user deletes the
//!    wrapped copy on the server and the next run has nothing to open.
//! 2. **The device private key is not the vault key.** On its own it opens nothing.
//! 3. **Every AAD label, KDF input and quantization rule here has a counterpart in the
//!    dashboard.** They must agree byte for byte; a divergence shows up as an
//!    authentication failure with no other clue, so each one names its counterpart.
//!
//! See `docs/PERSONAL_MEMORY_VAULT.md` in the MyraRouter repository for the design.
pub mod client;
pub mod crypto;
pub mod device;
pub mod search;
pub mod vault;

pub use client::ApiError;
pub use client::VaultApi;
pub use crypto::CryptoError;
pub use crypto::VaultKey;
pub use device::DeviceError;
pub use device::DeviceIdentity;
pub use search::SearchHit;
pub use vault::MemoryEntry;
pub use vault::MemoryPayload;
pub use vault::VaultError;
pub use vault::VaultIndex;
pub use vault::VaultSession;

#[cfg(test)]
mod tests;
