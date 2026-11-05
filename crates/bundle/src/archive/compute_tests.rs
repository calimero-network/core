//! A service's compute component rides in the signed manifest like every artifact.

use std::sync::Arc;

use flate2::write::GzEncoder;
use flate2::Compression;
use semver::Version;
use sha2::{Digest, Sha256};
use tar::{Builder, Header};

use crate::{sign_manifest_json, BundleArtifact, VerifiedBundle};

fn artifact(path: &str, bytes: &[u8]) -> serde_json::Value {
    let artifact = BundleArtifact {
        path: path.to_owned(),
        hash: hex::encode(Sha256::digest(bytes)),
        size: bytes.len() as u64,
    };
    serde_json::to_value(artifact).unwrap()
}

fn pack(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut tar = Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    for (name, bytes) in entries {
        let mut header = Header::new_gnu();
        header.set_path(name).unwrap();
        header.set_size(bytes.len() as u64);
        header.set_cksum();
        tar.append(&header, *bytes).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}

/// A signed bundle whose manifest is `base` plus what `edit` changes after signing's
/// inputs are set; `files` are the archive's entries.
fn bundle(manifest: serde_json::Value, files: &[(&str, &[u8])]) -> Arc<[u8]> {
    let key = ed25519_dalek::SigningKey::from_bytes(&[5; 32]);
    let mut manifest = manifest;
    sign_manifest_json(&mut manifest, &key).unwrap();
    let manifest = serde_json::to_vec(&manifest).unwrap();
    let mut entries = vec![("manifest.json", manifest.as_slice())];
    entries.extend_from_slice(files);
    pack(&entries).into()
}

fn manifest(extra: serde_json::Value) -> serde_json::Value {
    let mut m = serde_json::json!({
        "version": "1.0",
        "package": "com.example.compute",
        "appVersion": "1.0.0",
        "minRuntimeVersion": "0.1.0",
        "wasm": artifact("app.wasm", b"app"),
    });
    m.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    m
}

fn open(data: Arc<[u8]>) -> eyre::Result<VerifiedBundle> {
    VerifiedBundle::open(data, &Version::new(1, 0, 0))
}

#[test]
fn a_single_service_bundle_ships_its_compute_component() {
    let data = bundle(
        manifest(serde_json::json!({ "compute": artifact("compute.wasm", b"job") })),
        &[("app.wasm", b"app"), ("compute.wasm", b"job")],
    );
    let contents = open(data).unwrap().contents().unwrap();
    assert_eq!(&*contents.wasm[0].bytes, b"app");
    assert_eq!(contents.wasm[0].compute.as_deref(), Some(&b"job"[..]));
}

#[test]
fn a_multi_service_bundle_ships_a_compute_component_per_service() {
    let m = serde_json::json!({
        "version": "1.0",
        "package": "com.example.compute",
        "appVersion": "1.0.0",
        "minRuntimeVersion": "0.1.0",
        "services": [
            { "name": "a", "wasm": artifact("a.wasm", b"a"), "compute": artifact("a.compute.wasm", b"ja") },
            { "name": "b", "wasm": artifact("b.wasm", b"b") },
        ],
    });
    let data = bundle(
        m,
        &[
            ("a.wasm", b"a"),
            ("a.compute.wasm", b"ja"),
            ("b.wasm", b"b"),
        ],
    );
    let contents = open(data).unwrap().contents().unwrap();
    assert_eq!(contents.wasm[0].compute.as_deref(), Some(&b"ja"[..]));
    assert!(contents.wasm[1].compute.is_none());
}

#[test]
fn a_tampered_compute_artifact_is_refused() {
    let data = bundle(
        manifest(serde_json::json!({ "compute": artifact("compute.wasm", b"job") })),
        &[("app.wasm", b"app"), ("compute.wasm", b"JOB")],
    );
    let err = open(data).unwrap().contents().err().unwrap();
    assert!(format!("{err:#}").contains("compute.wasm"), "{err:#}");
}

#[test]
fn a_compute_artifact_added_after_signing_is_refused() {
    let key = ed25519_dalek::SigningKey::from_bytes(&[5; 32]);
    let mut signed = manifest(serde_json::json!({}));
    sign_manifest_json(&mut signed, &key).unwrap();
    signed["compute"] = artifact("compute.wasm", b"job");
    let manifest = serde_json::to_vec(&signed).unwrap();
    let data = pack(&[
        ("manifest.json", &manifest),
        ("app.wasm", b"app"),
        ("compute.wasm", b"job"),
    ]);
    assert!(open(data.into()).is_err());
}

#[test]
fn a_manifest_without_the_field_still_verifies() {
    let data = bundle(manifest(serde_json::json!({})), &[("app.wasm", b"app")]);
    let contents = open(data).unwrap().contents().unwrap();
    assert!(contents.wasm[0].compute.is_none());
}
