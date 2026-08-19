//! Drives a real Personal Memory vault end to end, the way MyraCode does.
//!
//! Exists to prove the cross-language contract rather than to assert it: a vault created and
//! written by the dashboard's JavaScript must be readable here, and what this writes must be
//! readable back there. AAD labels, HKDF inputs and the ECDH-ES envelope all have to agree byte
//! for byte, and nothing short of running both sides shows that.
//!
//!   cargo run -p codex-vault-client --example e2e_roundtrip -- <base_url> <api_key> <state_dir>
use std::path::PathBuf;

use codex_vault_client::DeviceIdentity;
use codex_vault_client::MemoryPayload;
use codex_vault_client::VaultApi;
use codex_vault_client::VaultError;
use codex_vault_client::VaultIndex;
use codex_vault_client::VaultSession;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let base_url = args.next().ok_or("usage: <base_url> <api_key> <state_dir>")?;
    let api_key = args.next().ok_or("missing api key")?;
    let state_dir = PathBuf::from(args.next().ok_or("missing state dir")?);

    let api = VaultApi::new(reqwest::Client::new(), &base_url, api_key);
    let mut identity = DeviceIdentity::load_or_create(&state_dir, "default").await?;
    println!("device fingerprint: {}", identity.fingerprint());

    let session = match VaultSession::open(api, &mut identity, None, "MyraCode e2e").await {
        Ok(session) => session,
        Err(VaultError::AwaitingApproval {
            pairing_code,
            fingerprint,
            vault_name,
        }) => {
            // Not a failure to retry blindly: nothing changes until a human approves it.
            println!("AWAITING_APPROVAL");
            println!("pairing_code={pairing_code}");
            println!("fingerprint={fingerprint}");
            println!("vault={vault_name}");
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };

    println!("opened vault: {} (key v{})", session.vault().name, session.vault().key_version);

    // Read what the dashboard wrote.
    let mut index = VaultIndex::new();
    let page = session.fetch_since(index.cursor()).await?;
    let mut tombstones = Vec::new();
    let mut decrypted = Vec::new();
    for row in &page.items {
        if row.deleted {
            tombstones.push(row.id.clone());
        } else {
            decrypted.push(session.decrypt_row(row));
        }
    }
    // A row that will not decrypt yields None, so count them rather than assume success.
    let unreadable = decrypted.iter().filter(|entry| entry.is_none()).count();
    index.apply(page.next_seq, decrypted, tombstones);

    println!("rows pulled: {}", page.items.len());
    println!("unreadable: {unreadable}");
    for entry in index.entries() {
        println!(
            "READ [{}] {} :: {}",
            entry.payload.source,
            entry.payload.title,
            entry.payload.body.chars().take(60).collect::<String>()
        );
        println!("     tags={:?}", entry.payload.tags);
    }

    // Write one back for the dashboard to read.
    let payload = MemoryPayload::new(
        "Vault client round trip",
        "Written by the Rust CLI. If the dashboard can read this, the AAD and key agreement hold.",
        vec!["e2e".to_string(), "myracode".to_string()],
        "2026-08-19T18:00:00.000Z".to_string(),
    );
    let written = session.write(payload, None).await?;
    println!("WROTE id={} seq={} rev={}", written.id, written.seq, written.rev);

    Ok(())
}
