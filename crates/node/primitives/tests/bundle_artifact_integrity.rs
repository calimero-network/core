//! Swapping `app.wasm` after signing leaves the manifest byte-identical, so the
//! per-artifact digest is the only thing that can catch a tampered bundle.

use std::fs;

use calimero_node_primitives::bundle::{derive_signer_id_did_key, sign_manifest_json};
use calimero_node_primitives::client::NodeClient;
use ed25519_dalek::SigningKey;
use flate2::write::GzEncoder;
use flate2::Compression;
use futures_util::io::Cursor;
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};
use tar::Builder;
use tempfile::TempDir;

mod common;

use common::{hex_lower, node_client, pack_entries};

/// Sign a manifest exactly the way `cargo mero bundle` does: real lowercase-hex
/// SHA-256 per artifact, then an Ed25519 signature over the canonical manifest.
fn signed_manifest(package: &str, version: &str, wasm: &[u8], key: &SigningKey) -> Vec<u8> {
    signed_manifest_at(package, version, "app.wasm", wasm, key)
}

fn signed_manifest_at(
    package: &str,
    version: &str,
    wasm_path: &str,
    wasm: &[u8],
    key: &SigningKey,
) -> Vec<u8> {
    let signer_id = derive_signer_id_did_key(key.verifying_key().as_bytes());
    let mut manifest = serde_json::json!({
        "version": "1.0",
        "package": package,
        "appVersion": version,
        "signerId": signer_id,
        "minRuntimeVersion": "0.1.0",
        "wasm": {
            "path": wasm_path,
            "size": wasm.len(),
            "hash": hex_lower(&Sha256::digest(wasm)),
        },
        "migrations": []
    });
    sign_manifest_json(&mut manifest, key).unwrap();
    serde_json::to_vec(&manifest).unwrap()
}

/// Pack a `.mpk` from an exact manifest byte string plus wasm bytes. Keeping the
/// manifest as raw bytes is the point: the attacker's copy is byte-identical.
fn pack(dir: &TempDir, name: &str, manifest_bytes: &[u8], wasm: &[u8]) -> Vec<u8> {
    let path = dir.path().join(name);
    let encoder = GzEncoder::new(fs::File::create(&path).unwrap(), Compression::default());
    let mut tar = Builder::new(encoder);

    for (entry_path, content) in [("manifest.json", manifest_bytes), ("app.wasm", wasm)] {
        let mut header = tar::Header::new_gnu();
        header.set_path(entry_path).unwrap();
        header.set_size(content.len() as u64);
        header.set_cksum();
        tar.append(&header, content).unwrap();
    }
    tar.finish().unwrap();
    drop(tar);
    fs::read(&path).unwrap()
}

/// Install through the production path (mandatory signature) and return the
/// ApplicationId plus the bytes the node would hand to the WASM runtime.
async fn install(
    client: &NodeClient,
    bundle: Vec<u8>,
) -> eyre::Result<(calimero_primitives::application::ApplicationId, Vec<u8>)> {
    let (blob_id, _) = client
        .add_blob(
            Cursor::new(bundle.as_slice()),
            Some(bundle.len() as u64),
            None,
        )
        .await?;
    let source = "https://registry.example.com/app.mpk".parse().unwrap();
    let app_id = client
        .install_application_from_bundle_blob(&blob_id, &source)
        .await?;
    let bytes = client
        .get_application_bytes(&app_id, None)
        .await?
        .expect("application bytes");
    Ok((app_id, bytes.to_vec()))
}

#[tokio::test]
async fn tampered_wasm_is_rejected_even_though_manifest_signature_verifies() {
    let dir = TempDir::new().unwrap();
    let key = SigningKey::generate(&mut OsRng);

    let honest_wasm = b"HONEST wasm bytecode from the publisher".as_slice();
    let evil_wasm = b"EVIL!! wasm bytecode from the registry ".as_slice();
    assert_eq!(
        honest_wasm.len(),
        evil_wasm.len(),
        "same length, so even manifest `size` still matches"
    );

    // One manifest, signed once. Both bundles carry these exact bytes.
    let manifest_bytes = signed_manifest("com.example.tamper", "1.0.0", honest_wasm, &key);
    let honest = pack(&dir, "honest.mpk", &manifest_bytes, honest_wasm);
    let evil = pack(&dir, "evil.mpk", &manifest_bytes, evil_wasm);

    // Separate nodes so neither install observes the other's state: both bundles
    // carry the same package and signer, hence the same application row.
    let (honest_node, _d1, _b1) = node_client().await;
    let (victim_node, _d2, _b2) = node_client().await;

    let (honest_id, honest_bytes) = install(&honest_node, honest).await.expect("honest install");
    assert_eq!(honest_bytes, honest_wasm);

    match install(&victim_node, evil).await {
        Err(err) => {
            let msg = err.to_string();
            assert!(
                msg.contains("app.wasm") && msg.contains("failed its integrity check"),
                "the digest must be what rejects it, not a missing or malformed artifact, got: {msg}"
            );
        }
        Ok((evil_id, bytes)) => panic!(
            "regression: a signed manifest admitted substituted bytecode.\n  \
             manifest.wasm.hash  = {}\n  \
             sha256(served wasm) = {}\n  \
             bytes served to the runtime = {:?}\n  \
             ApplicationId honest = {honest_id}\n  \
             ApplicationId evil   = {evil_id}  (same = {})",
            hex_lower(&Sha256::digest(honest_wasm)),
            hex_lower(&Sha256::digest(&bytes)),
            String::from_utf8_lossy(&bytes),
            honest_id == evil_id,
        ),
    }
}

/// A manifest with no per-artifact hash must not parse: the field is what binds
/// bytes to the signature, so its absence is a malformed bundle, not a default.
#[tokio::test]
async fn manifest_without_artifact_hash_is_rejected() {
    let dir = TempDir::new().unwrap();
    let key = SigningKey::generate(&mut OsRng);
    let wasm = b"some wasm".as_slice();

    let signer_id = derive_signer_id_did_key(key.verifying_key().as_bytes());
    let mut manifest = serde_json::json!({
        "version": "1.0",
        "package": "com.example.nohash",
        "appVersion": "1.0.0",
        "signerId": signer_id,
        "minRuntimeVersion": "0.1.0",
        "wasm": { "path": "app.wasm", "size": wasm.len() },
        "migrations": []
    });
    sign_manifest_json(&mut manifest, &key).unwrap();
    let bundle = pack(
        &dir,
        "nohash.mpk",
        &serde_json::to_vec(&manifest).unwrap(),
        wasm,
    );

    let (node, _d, _b) = node_client().await;
    let err = install(&node, bundle)
        .await
        .expect_err("a hashless manifest must not install");
    // Must be refused while parsing: a defaulted hash would fail the digest check
    // too, so "it errored" alone would not catch the field turning optional again.
    let msg = err.to_string();
    assert!(
        msg.contains("hash") && !msg.contains("failed its integrity check"),
        "a hashless manifest must be rejected as malformed, got: {msg}"
    );
}

/// The manifest is read before its signature can be checked, so an unbounded
/// read is a pre-auth decompression bomb. This manifest is otherwise valid and
/// correctly signed: without the cap it installs, so only the cap can reject it.
#[tokio::test]
async fn oversized_manifest_is_refused_before_any_signature_check() {
    let dir = TempDir::new().unwrap();
    let key = SigningKey::generate(&mut OsRng);
    let wasm = b"wasm behind an oversized manifest".as_slice();

    let signer_id = derive_signer_id_did_key(key.verifying_key().as_bytes());
    let mut manifest = serde_json::json!({
        "version": "1.0",
        "package": "com.example.bigmanifest",
        "appVersion": "1.0.0",
        "signerId": signer_id,
        "minRuntimeVersion": "0.1.0",
        "metadata": { "name": "big", "description": "p".repeat(1024 * 1024) },
        "wasm": {
            "path": "app.wasm",
            "size": wasm.len(),
            "hash": hex_lower(&Sha256::digest(wasm)),
        },
        "migrations": []
    });
    sign_manifest_json(&mut manifest, &key).unwrap();
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
    assert!(
        manifest_bytes.len() > 1024 * 1024,
        "the fixture must exceed the cap, got {} bytes",
        manifest_bytes.len()
    );
    let bundle = pack(&dir, "bigmanifest.mpk", &manifest_bytes, wasm);

    let (node, _d, _b) = node_client().await;
    let err = install(&node, bundle)
        .await
        .expect_err("an oversized manifest must not install");
    // Pinned to the cap's own wording: a signature or parse rejection, which a
    // truncated read would also produce, must not satisfy this assertion.
    let msg = err.to_string();
    assert!(
        msg.contains("manifest.json exceeds the 1048576 byte cap"),
        "must be refused by the manifest size cap, got: {msg}"
    );
}

/// The cap must not touch manifests of any realistic size.
#[tokio::test]
async fn normal_manifest_still_installs() {
    let dir = TempDir::new().unwrap();
    let key = SigningKey::generate(&mut OsRng);
    let wasm = b"wasm behind a normal manifest".as_slice();
    let manifest = signed_manifest("com.example.normal", "1.0.0", wasm, &key);
    let bundle = pack(&dir, "normal.mpk", &manifest, wasm);

    let (node, _d, _b) = node_client().await;
    let (_app_id, served) = install(&node, bundle).await.expect("install");
    assert_eq!(served, wasm);
}

/// Reading each artifact path once is what stops N services naming one large
/// artifact from costing N copies, so the shared read must still hand every
/// service its own artifact's bytes.
#[tokio::test]
async fn services_sharing_an_artifact_path_each_get_the_right_bytes() {
    let dir = TempDir::new().unwrap();
    let key = SigningKey::generate(&mut OsRng);
    let a = b"wasm for service a".as_slice();
    let b = b"wasm for service b, distinct".as_slice();

    let signer_id = derive_signer_id_did_key(key.verifying_key().as_bytes());
    let artifact = |path: &str, bytes: &[u8]| {
        serde_json::json!({
            "path": path,
            "size": bytes.len(),
            "hash": hex_lower(&Sha256::digest(bytes)),
        })
    };
    let mut manifest = serde_json::json!({
        "version": "1.0",
        "package": "com.example.shared",
        "appVersion": "1.0.0",
        "signerId": signer_id,
        "minRuntimeVersion": "0.1.0",
        // `c` deliberately names `a`'s path: one read, two services.
        "services": [
            { "name": "a", "wasm": artifact("a.wasm", a) },
            { "name": "b", "wasm": artifact("b.wasm", b) },
            { "name": "c", "wasm": artifact("a.wasm", a) },
        ],
        "migrations": []
    });
    sign_manifest_json(&mut manifest, &key).unwrap();
    let bundle = pack_entries(
        &dir,
        "shared.mpk",
        &[
            ("manifest.json", &serde_json::to_vec(&manifest).unwrap()),
            ("a.wasm", a),
            ("b.wasm", b),
        ],
    );

    let (node, _d, _b) = node_client().await;
    let (blob_id, _) = node
        .add_blob(
            Cursor::new(bundle.as_slice()),
            Some(bundle.len() as u64),
            None,
        )
        .await
        .unwrap();
    let source = "https://registry.example.com/app.mpk".parse().unwrap();
    let app_id = node
        .install_application_from_bundle_blob(&blob_id, &source)
        .await
        .expect("multi-service install");

    // The per-service bytecode blob is what the shared read decides, so it is
    // what pins the keying; the served bytes are re-read and digest-checked and
    // would look right even if a service had been registered against the wrong blob.
    let app = node.get_application(&app_id).unwrap().expect("application");
    for (service, expected) in [("a", a), ("b", b), ("c", a)] {
        let blob = app
            .services
            .get(service)
            .unwrap_or_else(|| panic!("service '{service}' missing"))
            .bytecode;
        let stored = node
            .get_blob_bytes(&blob, None)
            .await
            .unwrap()
            .expect("service blob bytes");
        assert_eq!(
            stored.as_ref(),
            expected,
            "service '{service}' was registered against another artifact's blob"
        );

        let served = node
            .get_application_bytes(&app_id, Some(service))
            .await
            .unwrap()
            .expect("service bytes");
        assert_eq!(
            served.as_ref(),
            expected,
            "service '{service}' was served another artifact's bytes"
        );
    }
}

/// Sharing one read between services must not share one digest check: the
/// second service declares a different hash for the same path, and its own
/// check is the only thing that can catch it.
#[tokio::test]
async fn a_shared_path_is_checked_against_every_service_digest() {
    let dir = TempDir::new().unwrap();
    let key = SigningKey::generate(&mut OsRng);
    let wasm = b"the one artifact both services name".as_slice();
    let other = b"bytes no entry in this archive holds".as_slice();

    let signer_id = derive_signer_id_did_key(key.verifying_key().as_bytes());
    let mut manifest = serde_json::json!({
        "version": "1.0",
        "package": "com.example.mixedhash",
        "appVersion": "1.0.0",
        "signerId": signer_id,
        "minRuntimeVersion": "0.1.0",
        // Same path, two digests. `a` matches the archive, `b` does not.
        "services": [
            { "name": "a", "wasm": {
                "path": "app.wasm",
                "size": wasm.len(),
                "hash": hex_lower(&Sha256::digest(wasm)),
            } },
            { "name": "b", "wasm": {
                "path": "app.wasm",
                "size": wasm.len(),
                "hash": hex_lower(&Sha256::digest(other)),
            } },
        ],
        "migrations": []
    });
    sign_manifest_json(&mut manifest, &key).unwrap();
    let bundle = pack_entries(
        &dir,
        "mixedhash.mpk",
        &[
            ("manifest.json", &serde_json::to_vec(&manifest).unwrap()),
            ("app.wasm", wasm),
        ],
    );

    let (node, _d, _b) = node_client().await;
    let err = install(&node, bundle)
        .await
        .expect_err("a service whose declared digest does not match must not install");
    // Pinned to the digest check: a missing-artifact or parse rejection would
    // mean the second service's hash was never compared at all.
    let msg = err.to_string();
    assert!(
        msg.contains("app.wasm") && msg.contains("failed its integrity check"),
        "the second service's digest must be the thing that fails, got: {msg}"
    );
}

/// `is_bundle_blob` is the only signature gate on the execution-time read, so it
/// must answer for the same archive the verified read parses. A manifest sitting
/// past a peeked prefix installed verified, then served the raw archive instead.
#[tokio::test]
async fn manifest_past_the_tenth_entry_is_a_bundle_on_both_paths() {
    let dir = TempDir::new().unwrap();
    let key = SigningKey::generate(&mut OsRng);
    let wasm = b"wasm behind eleven leading entries".as_slice();
    let manifest = signed_manifest("com.example.latemanifest", "1.0.0", wasm, &key);

    let padding: Vec<String> = (0..10).map(|i| format!("pad/{i}.txt")).collect();
    let mut entries: Vec<(&str, &[u8])> = padding
        .iter()
        .map(|p| (p.as_str(), b"pad".as_slice()))
        .collect();
    entries.push(("manifest.json", &manifest));
    entries.push(("app.wasm", wasm));
    let bundle = pack_entries(&dir, "late.mpk", &entries);

    assert!(
        NodeClient::is_bundle_blob(&bundle),
        "detection must find the same manifest the verified read finds"
    );

    let (node, _d, _b) = node_client().await;
    let (_app_id, served) = install(&node, bundle).await.expect("install");
    assert_eq!(
        served, wasm,
        "the served bytes must be the digest-checked artifact, not the raw archive"
    );
}

/// The bytes whose digest was checked must be the bytes the node later serves: a
/// decoy sharing the manifest path's basename must not satisfy the check for it.
#[tokio::test]
async fn decoy_entry_cannot_satisfy_the_digest_check_for_another_entry() {
    let dir = TempDir::new().unwrap();
    let key = SigningKey::generate(&mut OsRng);

    let honest = b"HONEST wasm bytecode from the publisher".as_slice();
    let evil = b"EVIL!! wasm bytecode from the registry ".as_slice();

    // Signed manifest commits to the honest bytes at path "app.wasm".
    let manifest = signed_manifest("com.example.decoy", "1.0.0", honest, &key);

    // The decoy sits BEFORE the real path so a first-match-by-basename lookup
    // hashes it, while an unpack-everything writer lets the later entry win.
    let bundle = pack_entries(
        &dir,
        "decoy.mpk",
        &[
            ("manifest.json", &manifest),
            ("decoy/app.wasm", honest),
            ("app.wasm", evil),
        ],
    );

    let (node, _d, _b) = node_client().await;
    match install(&node, bundle).await {
        // The decoy must be skipped outright, leaving the real `app.wasm` hashed
        // and refused; any other rejection means the decoy was still reached.
        Err(err) => {
            let msg = err.to_string();
            assert!(
                msg.contains("app.wasm") && msg.contains("failed its integrity check"),
                "the decoy must be ignored and the real entry must fail its digest, got: {msg}"
            );
        }
        Ok((_app_id, served)) => assert_eq!(
            served,
            honest,
            "BYPASS: digest checked the decoy entry but the node served {:?}",
            String::from_utf8_lossy(&served),
        ),
    }
}

/// The content-addressed blob is the only copy of a bundle's bytes. Nothing is
/// unpacked to disk, so there is no second copy to go stale or be tampered with.
#[tokio::test]
async fn install_writes_no_extracted_directory() {
    let dir = TempDir::new().unwrap();
    let key = SigningKey::generate(&mut OsRng);
    let wasm = b"wasm for the no-extract check".as_slice();
    let manifest = signed_manifest("com.example.noextract", "1.0.0", wasm, &key);
    let bundle = pack(&dir, "noextract.mpk", &manifest, wasm);

    let (node, _data, blob_dir) = node_client().await;
    let (_app_id, served) = install(&node, bundle).await.expect("install");
    assert_eq!(served, wasm, "the blob must still serve the right bytes");

    let applications = blob_dir.path().join("applications");
    assert!(
        !applications.exists(),
        "no artifacts should be unpacked to disk, found {}",
        applications.display()
    );
}

/// The rollback in its bare form: the archive's only manifest is nested, and it
/// is authentically signed, so nothing but the exact path can refuse it.
#[tokio::test]
async fn a_nested_manifest_is_never_the_bundle_manifest() {
    let dir = TempDir::new().unwrap();
    let key = SigningKey::generate(&mut OsRng);
    let old_wasm = b"wasm bytecode of the superseded release".as_slice();
    let old = signed_manifest_at(
        "com.example.nested",
        "1.0.0",
        "old/app.wasm",
        old_wasm,
        &key,
    );

    let bundle = pack_entries(
        &dir,
        "nested.mpk",
        &[("old/manifest.json", &old), ("old/app.wasm", old_wasm)],
    );

    let (node, _d, _b) = node_client().await;
    let err = install(&node, bundle)
        .await
        .expect_err("a bundle whose only manifest is nested must not install");
    assert!(
        err.to_string()
            .contains("manifest.json not found in bundle"),
        "the nested manifest must not be selected at all, got: {err}"
    );
}

/// A nested `old/manifest.json` holds a genuinely signed older release, so no
/// signature check can catch it: only selecting the exact top-level path can.
/// Both manifests here install cleanly, so which one wins is the whole test.
#[tokio::test]
async fn a_nested_manifest_cannot_roll_the_install_back_to_an_older_release() {
    let dir = TempDir::new().unwrap();
    let key = SigningKey::generate(&mut OsRng);

    let old_wasm = b"wasm bytecode of the superseded release".as_slice();
    let new_wasm = b"wasm bytecode of the current release !!".as_slice();

    // Same publisher key, each manifest committing to its own artifact.
    let old = signed_manifest_at(
        "com.example.rollback",
        "1.0.0",
        "old/app.wasm",
        old_wasm,
        &key,
    );
    let new = signed_manifest_at("com.example.rollback", "2.0.0", "app.wasm", new_wasm, &key);

    // The old pair leads, so a basename-and-first-match selection installs 1.0.0.
    let bundle = pack_entries(
        &dir,
        "rollback.mpk",
        &[
            ("old/manifest.json", &old),
            ("old/app.wasm", old_wasm),
            ("manifest.json", &new),
            ("app.wasm", new_wasm),
        ],
    );

    let (node, _d, _b) = node_client().await;
    let (app_id, served) = install(&node, bundle).await.expect("install");
    assert_eq!(
        served, new_wasm,
        "ROLLBACK: the nested manifest was selected and the superseded release installed"
    );
    let app = node.get_application(&app_id).unwrap().expect("application");
    assert_eq!(
        app.version.map(|v| v.to_string()).as_deref(),
        Some("2.0.0"),
        "the installed version must come from the top-level manifest"
    );
}

/// Two entries at the manifest's own path: whichever one an unpacker keeps, the
/// signature was checked against the other. Both are authentic, so nothing but
/// the duplicate rule can separate them.
#[tokio::test]
async fn duplicate_top_level_manifest_entries_are_refused() {
    let dir = TempDir::new().unwrap();
    let key = SigningKey::generate(&mut OsRng);
    let wasm = b"wasm behind two manifests".as_slice();

    let first = signed_manifest("com.example.twomanifests", "1.0.0", wasm, &key);
    let second = signed_manifest("com.example.twomanifests", "2.0.0", wasm, &key);

    let bundle = pack_entries(
        &dir,
        "twomanifests.mpk",
        &[
            ("manifest.json", &first),
            ("app.wasm", wasm),
            ("manifest.json", &second),
        ],
    );

    let (node, _d, _b) = node_client().await;
    let err = install(&node, bundle)
        .await
        .expect_err("a duplicated manifest entry must not install");
    assert!(
        err.to_string()
            .contains("more than one entry at 'manifest.json'"),
        "the duplicate must be refused as such, not left to entry order, got: {err}"
    );
}

/// Finding the duplicate means walking to the end of the archive, so the cap
/// that bounds the pre-signature manifest scan must not bound the whole bundle.
#[tokio::test]
async fn a_bundle_larger_than_the_manifest_scan_cap_still_installs() {
    let dir = TempDir::new().unwrap();
    let key = SigningKey::generate(&mut OsRng);

    // Past the 8 MiB scan cap, and compressible enough to stay quick.
    let wasm = vec![0xABu8; 12 * 1024 * 1024];
    let manifest = signed_manifest("com.example.large", "1.0.0", &wasm, &key);
    let bundle = pack_entries(
        &dir,
        "large.mpk",
        &[("manifest.json", &manifest), ("app.wasm", &wasm)],
    );

    let (node, _d, _b) = node_client().await;
    let (_app_id, served) = install(&node, bundle).await.expect("install");
    assert!(
        served == wasm,
        "a bundle whose artifacts outweigh the manifest scan cap must still install"
    );
}

/// Two entries at the manifest's path: whichever one an unpacker keeps, the
/// digest check saw the other. The archive is refused outright.
#[tokio::test]
async fn duplicate_entries_at_the_manifest_path_are_refused() {
    let dir = TempDir::new().unwrap();
    let key = SigningKey::generate(&mut OsRng);

    let honest = b"HONEST wasm bytecode from the publisher".as_slice();
    let evil = b"EVIL!! wasm bytecode from the registry ".as_slice();
    let manifest = signed_manifest("com.example.dup", "1.0.0", honest, &key);

    let bundle = pack_entries(
        &dir,
        "dup.mpk",
        &[
            ("manifest.json", &manifest),
            ("app.wasm", honest),
            ("app.wasm", evil),
        ],
    );

    let (node, _d, _b) = node_client().await;
    let err = install(&node, bundle)
        .await
        .expect_err("a duplicated artifact entry must not install");
    assert!(
        err.to_string()
            .contains("more than one entry at 'app.wasm'"),
        "the duplicate must be refused as such, not left to hash ordering, got: {err}"
    );
}
