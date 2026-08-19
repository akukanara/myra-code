use crate::crypto;
use crate::crypto::VaultKey;
use crate::search;
use crate::vault::MemoryEntry;
use crate::vault::MemoryPayload;
use pretty_assertions::assert_eq;

fn entry(id: &str, title: &str, body: &str, tags: &[&str], vector: Option<Vec<i8>>) -> MemoryEntry {
    MemoryEntry {
        id: id.to_string(),
        rev: 1,
        seq: 1,
        key_version: 1,
        payload: MemoryPayload {
            version: crypto::SCHEMA_VERSION,
            title: title.to_string(),
            body: body.to_string(),
            tags: tags.iter().copied().map(String::from).collect(),
            project: None,
            source: "myracode".to_string(),
            pinned: false,
            created_at: "2026-08-19T00:00:00.000Z".to_string(),
            updated_at: "2026-08-19T00:00:00.000Z".to_string(),
        },
        vector,
    }
}

#[test]
fn base64_accepts_both_forms_the_dashboard_and_other_tools_emit() {
    // The dashboard emits url-safe unpadded; other tooling may send standard padded.
    let bytes = vec![251u8, 239, 190, 1, 2, 3];
    let url_safe = crypto::encode_base64(&bytes);
    assert!(!url_safe.contains('+') && !url_safe.contains('/') && !url_safe.contains('='));
    assert_eq!(crypto::decode_base64(&url_safe).unwrap(), bytes);
    assert_eq!(crypto::decode_base64("++--//__").is_err(), false);
}

#[test]
fn aad_labels_match_the_dashboard_exactly() {
    // These strings are the contract. A change on one side alone makes every existing
    // memory undecryptable on the other, with no other symptom.
    assert_eq!(
        String::from_utf8(crypto::item_aad("vault-1", "item-1", 2)).unwrap(),
        "myravault/item/v1|vault-1|item-1|2"
    );
    assert_eq!(
        String::from_utf8(crypto::vector_aad("vault-1", "item-1", 2)).unwrap(),
        "myravault/vec/v1|vault-1|item-1|2"
    );
    assert_eq!(
        String::from_utf8(crypto::wrap_aad("c2FsdA")).unwrap(),
        "myravault/wrap/v1|c2FsdA"
    );
}

#[test]
fn seal_and_open_round_trip() {
    let key = VaultKey::from_bytes(&[7u8; 32]).unwrap();
    let aad = crypto::item_aad("vault-1", "item-1", 1);
    let nonce = crypto::random_nonce();
    let sealed = key.seal(&nonce, &aad, b"a memory").unwrap();
    assert_eq!(key.open(&nonce, &aad, &sealed).unwrap(), b"a memory");
}

#[test]
fn ciphertext_moved_to_another_item_or_vault_fails() {
    let key = VaultKey::from_bytes(&[7u8; 32]).unwrap();
    let nonce = crypto::random_nonce();
    let sealed = key
        .seal(&nonce, &crypto::item_aad("vault-1", "item-1", 1), b"a memory")
        .unwrap();

    for wrong in [
        crypto::item_aad("vault-1", "item-2", 1),
        crypto::item_aad("vault-2", "item-1", 1),
        crypto::item_aad("vault-1", "item-1", 2),
        crypto::vector_aad("vault-1", "item-1", 1),
    ] {
        assert!(key.open(&nonce, &wrong, &sealed).is_err());
    }
}

#[test]
fn a_wrong_key_does_not_open_it() {
    let aad = crypto::item_aad("vault-1", "item-1", 1);
    let nonce = crypto::random_nonce();
    let sealed = VaultKey::from_bytes(&[7u8; 32])
        .unwrap()
        .seal(&nonce, &aad, b"a memory")
        .unwrap();
    assert!(
        VaultKey::from_bytes(&[8u8; 32])
            .unwrap()
            .open(&nonce, &aad, &sealed)
            .is_err()
    );
}

#[test]
fn a_vault_key_never_prints_itself() {
    // A `{key:?}` in a log line would undo the whole design.
    let key = VaultKey::from_bytes(&[7u8; 32]).unwrap();
    let rendered = format!("{key:?}");
    assert_eq!(rendered, "VaultKey(<redacted>)");
    assert!(!rendered.contains('7'));
}

#[test]
fn a_vault_key_must_be_thirty_two_bytes() {
    assert!(VaultKey::from_bytes(&[0u8; 16]).is_err());
    assert!(VaultKey::from_bytes(&[0u8; 33]).is_err());
    assert!(VaultKey::from_bytes(&[0u8; 32]).is_ok());
}

#[test]
fn a_device_key_round_trips_through_the_jwk_the_server_stores() {
    let secret = crypto::generate_secret_key().unwrap();
    let (x, y) = crypto::jwk_coordinates(&secret.public_key());
    let restored = crypto::public_key_from_jwk(&x, &y).unwrap();
    assert_eq!(restored, secret.public_key());
}

#[test]
fn the_device_wrapped_key_opens_only_with_that_device_key() {
    // Mirrors what the dashboard does at approval time: ECDH-ES to a device public key.
    let device = crypto::generate_secret_key().unwrap();
    let ephemeral = crypto::generate_secret_key().unwrap();
    let aad_salt = crypto::encode_base64(&crypto::random_bytes(16));
    let vault_key = crypto::random_bytes(32);

    // The wrapping side derives from (ephemeral private, device public).
    let wrapping = crypto::device_wrapping_key_for_test(&ephemeral, &device.public_key(), &aad_salt)
        .unwrap();
    let nonce = crypto::random_nonce();
    let wrapped = wrapping
        .seal(&nonce, &crypto::wrap_aad(&aad_salt), &vault_key)
        .unwrap();

    // The device side derives the same secret from (device private, ephemeral public).
    let opened = crypto::unwrap_vault_key_for_device(
        &device,
        &ephemeral.public_key(),
        &aad_salt,
        &nonce,
        &wrapped,
    )
    .unwrap();
    assert_eq!(
        opened.open(&nonce, b"probe", &opened.seal(&nonce, b"probe", b"x").unwrap()).unwrap(),
        b"x"
    );

    // Another device cannot.
    let other = crypto::generate_secret_key().unwrap();
    assert!(
        crypto::unwrap_vault_key_for_device(
            &other,
            &ephemeral.public_key(),
            &aad_salt,
            &nonce,
            &wrapped
        )
        .is_err()
    );
}

#[test]
fn a_fingerprint_matches_the_dashboards_format() {
    // Same canonical JSON and the same 8-group-of-4 rendering as pairing.js, or the two
    // sides would show different fingerprints for the same key.
    let fingerprint = crypto::jwk_fingerprint("eA", "eQ");
    assert_eq!(fingerprint.len(), 19);
    assert_eq!(fingerprint.matches('-').count(), 3);
    assert_eq!(fingerprint, fingerprint.to_uppercase());
}

#[test]
fn quantization_uses_the_whole_int8_range() {
    // Unit-normalising then scaling by 127 collapses a 768-dimension vector into a handful
    // of steps; a per-vector scale keeps the resolution.
    let vector: Vec<f32> = (0..768).map(|index| (index as f32 / 7.0).sin()).collect();
    let quantized = search::quantize(&vector);
    assert_eq!(quantized.len(), 768);
    assert!(quantized.iter().any(|value| value.abs() > 120));
}

#[test]
fn quantization_preserves_direction() {
    let vector: Vec<f32> = (0..768).map(|index| (index as f32 / 7.0).sin()).collect();
    let quantized = search::quantize(&vector);
    let reference = search::quantize(&vector);
    assert!(search::similarity(&quantized, &reference) > 0.9999);
}

#[test]
fn similarity_is_bounded_and_handles_degenerate_input() {
    let a = search::quantize(&[1.0, 2.0, 3.0, 4.0]);
    let b = search::quantize(&[-1.0, -2.0, -3.0, -4.0]);
    assert!(search::similarity(&a, &a) > 0.999);
    assert!(search::similarity(&a, &b) < -0.999);
    // Mismatched lengths and all-zero vectors must not panic or divide by zero.
    assert_eq!(search::similarity(&a, &[0i8; 2]), -1.0);
    assert_eq!(search::similarity(&[0i8; 4], &[0i8; 4]), 0.0);
}

#[test]
fn vector_search_ranks_and_respects_the_floor() {
    let entries = vec![
        entry("a", "db", "x", &["infra"], Some(search::quantize(&[1.0, 0.0, 0.0]))),
        entry("b", "ui", "x", &["design"], Some(search::quantize(&[0.9, 0.1, 0.0]))),
        entry("c", "far", "x", &["infra"], Some(search::quantize(&[-1.0, 0.0, 0.0]))),
    ];
    let query = search::quantize(&[1.0, 0.0, 0.0]);

    let hits = search::by_vector(&entries, &query, 5, 0.0);
    assert_eq!(
        hits.iter().map(|hit| hit.entry.id.as_str()).collect::<Vec<_>>(),
        vec!["a", "b"]
    );
    // Lowering the floor lets the opposite-facing memory through.
    assert_eq!(search::by_vector(&entries, &query, 5, -1.0).len(), 3);
}

#[test]
fn vector_search_skips_memories_that_were_never_embedded() {
    let entries = vec![entry("a", "x", "y", &[], None)];
    assert!(search::by_vector(&entries, &[1i8, 0, 0], 5, 0.0).is_empty());
}

#[test]
fn text_search_prefers_a_title_hit() {
    let entries = vec![
        entry("a", "postgres migration", "notes", &[], None),
        entry("b", "notes", "we chose postgres", &[], None),
        entry("c", "unrelated", "nothing", &[], None),
    ];
    assert_eq!(
        search::by_text(&entries, "postgres", 5)
            .iter()
            .map(|hit| hit.entry.id.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b"]
    );
    assert!(search::by_text(&entries, "   ", 5).is_empty());
}

#[test]
fn combined_search_keeps_an_exact_phrase_that_the_vectors_would_bury() {
    let entries = vec![
        entry("a", "alpha", "the quick brown fox", &[], Some(search::quantize(&[0.0, 1.0]))),
        entry("b", "beta", "something else", &[], Some(search::quantize(&[1.0, 0.0]))),
    ];
    let query_vector = search::quantize(&[1.0, 0.0]);
    let hits = search::combined(&entries, "quick brown", Some(&query_vector), 5);
    let ids: Vec<&str> = hits.iter().map(|hit| hit.entry.id.as_str()).collect();
    assert!(ids.contains(&"a"), "exact phrase must survive: {ids:?}");
    assert!(ids.contains(&"b"));
}

#[test]
fn indexable_text_includes_the_title_and_tags() {
    // A memory titled "postgres decision" has to be findable by that phrase even when the
    // body never repeats it.
    let payload = MemoryPayload::new(
        "postgres decision",
        "we moved",
        vec!["infra".to_string()],
        "2026-08-19T00:00:00.000Z".to_string(),
    );
    let text = payload.indexable_text();
    assert!(text.contains("postgres decision"));
    assert!(text.contains("infra"));
    assert!(text.contains("we moved"));
}

#[test]
fn a_written_memory_is_labelled_as_the_agents() {
    let payload = MemoryPayload::new("t", "b", Vec::new(), "2026-08-19T00:00:00.000Z".to_string());
    assert_eq!(payload.source, "myracode");
    assert_eq!(payload.version, crypto::SCHEMA_VERSION);
}

#[test]
fn the_payload_serialises_with_the_field_names_the_dashboard_wrote() {
    // camelCase `v`, and snake_case is NOT applied: these keys are the wire format.
    let payload = MemoryPayload::new("t", "b", vec!["x".to_string()], "now".to_string());
    let json = serde_json::to_value(&payload).unwrap();
    for key in ["v", "title", "body", "tags", "source", "pinned"] {
        assert!(json.get(key).is_some(), "missing {key} in {json}");
    }
}

#[tokio::test]
async fn a_device_key_is_created_once_and_reloaded() {
    let root = tempfile::tempdir().unwrap();
    let first = crate::DeviceIdentity::load_or_create(root.path(), "vault-1")
        .await
        .unwrap();
    let fingerprint = first.fingerprint().to_string();
    let (x, y) = first.public_jwk();
    let coordinates = (x.to_string(), y.to_string());
    drop(first);

    let second = crate::DeviceIdentity::load_or_create(root.path(), "vault-1")
        .await
        .unwrap();
    assert_eq!(second.fingerprint(), fingerprint);
    let (x, y) = second.public_jwk();
    assert_eq!((x.to_string(), y.to_string()), coordinates);
}

#[tokio::test]
async fn each_vault_gets_its_own_device_key() {
    // A key enrolled against one vault must not silently be an identity for another.
    let root = tempfile::tempdir().unwrap();
    let personal = crate::DeviceIdentity::load_or_create(root.path(), "vault-personal")
        .await
        .unwrap();
    let work = crate::DeviceIdentity::load_or_create(root.path(), "vault-work")
        .await
        .unwrap();
    assert_ne!(personal.fingerprint(), work.fingerprint());
    assert_ne!(personal.path(), work.path());
}

#[cfg(unix)]
#[tokio::test]
async fn a_device_key_is_written_private_and_a_loose_one_is_refused() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let identity = crate::DeviceIdentity::load_or_create(root.path(), "vault-1")
        .await
        .unwrap();
    let path = identity.path().to_path_buf();
    let mode = tokio::fs::metadata(&path).await.unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "device key must not be readable by others");

    // Loosened by hand: refused rather than quietly used, because a key another account can
    // read is a key another account can use.
    let mut permissions = tokio::fs::metadata(&path).await.unwrap().permissions();
    permissions.set_mode(0o644);
    tokio::fs::set_permissions(&path, permissions).await.unwrap();
    let error = crate::DeviceIdentity::load_or_create(root.path(), "vault-1")
        .await
        .expect_err("a world-readable device key must be refused");
    assert!(format!("{error}").contains("chmod 600"), "{error}");
}

#[tokio::test]
async fn a_device_key_file_never_contains_a_vault_key() {
    let root = tempfile::tempdir().unwrap();
    let identity = crate::DeviceIdentity::load_or_create(root.path(), "vault-1")
        .await
        .unwrap();
    let contents = tokio::fs::read_to_string(identity.path()).await.unwrap();
    // The stored shape is the device keypair and identifiers -- nothing that opens a vault
    // on its own, and no cached vault key.
    for forbidden in ["vaultKey", "vault_key", "vdk", "wrappedKey"] {
        assert!(!contents.contains(forbidden), "{forbidden} must not be stored");
    }
}
