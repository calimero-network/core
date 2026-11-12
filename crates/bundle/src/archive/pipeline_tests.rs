//! A package's pipelines ride in the signed manifest like every artifact.

use std::sync::Arc;

use flate2::write::GzEncoder;
use flate2::Compression;
use semver::Version;
use sha2::{Digest, Sha256};
use tar::{Builder, Header};

use crate::{
    sign_manifest_json, BundleArtifact, BundleManifest, VerifiedBundle, MAX_PIPELINES_PER_PACKAGE,
};

fn artifact(path: &str, bytes: &[u8]) -> serde_json::Value {
    serde_json::to_value(BundleArtifact {
        path: path.to_owned(),
        hash: hex::encode(Sha256::digest(bytes)),
        size: bytes.len() as u64,
    })
    .unwrap()
}

fn bundle(manifest: serde_json::Value, files: &[(&str, &[u8])]) -> Arc<[u8]> {
    let key = ed25519_dalek::SigningKey::from_bytes(&[5; 32]);
    let mut manifest = manifest;
    sign_manifest_json(&mut manifest, &key).unwrap();
    let manifest = serde_json::to_vec(&manifest).unwrap();
    let mut tar = Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    for (name, bytes) in std::iter::once(&("manifest.json", manifest.as_slice())).chain(files) {
        let mut header = Header::new_gnu();
        header.set_path(name).unwrap();
        header.set_size(bytes.len() as u64);
        header.set_cksum();
        tar.append(&header, *bytes).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap().into()
}

fn manifest(extra: serde_json::Value) -> serde_json::Value {
    let mut m = serde_json::json!({
        "version": "1.0",
        "package": "com.example.pipelines",
        "appVersion": "1.0.0",
        "minRuntimeVersion": "0.1.0",
        "compute": artifact("compute.wasm", b"job"),
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
fn a_manifest_with_pipelines_round_trips() {
    let m = manifest(serde_json::json!({ "pipelines": [artifact("pipelines/a.yaml", b"a: 1")] }));
    let parsed: BundleManifest = serde_json::from_value(m).unwrap();
    assert_eq!(parsed.pipelines.len(), 1);
    let back = serde_json::to_value(&parsed).unwrap();
    assert_eq!(back["pipelines"][0]["path"], "pipelines/a.yaml");
    let plain: BundleManifest = serde_json::from_value(manifest(serde_json::json!({}))).unwrap();
    assert!(serde_json::to_value(&plain)
        .unwrap()
        .get("pipelines")
        .is_none());
}

#[test]
fn a_bundle_ships_its_pipeline_files() {
    let data = bundle(
        manifest(serde_json::json!({ "pipelines": [artifact("pipelines/a.yaml", b"a: 1")] })),
        &[("compute.wasm", b"job"), ("pipelines/a.yaml", b"a: 1")],
    );
    let contents = open(data).unwrap().contents().unwrap();
    assert_eq!(contents.wasm[0].pipelines.len(), 1);
    assert_eq!(contents.wasm[0].pipelines[0].0, "pipelines/a.yaml");
    assert_eq!(&*contents.wasm[0].pipelines[0].1, b"a: 1");
}

#[test]
fn a_service_ships_its_pipeline_files() {
    let m = serde_json::json!({
        "version": "1.0",
        "package": "com.example.pipelines",
        "appVersion": "1.0.0",
        "minRuntimeVersion": "0.1.0",
        "services": [{
            "name": "a",
            "wasm": artifact("a.wasm", b"a"),
            "pipelines": [artifact("p.yaml", b"p: 1")],
        }],
    });
    let data = bundle(m, &[("a.wasm", b"a"), ("p.yaml", b"p: 1")]);
    let contents = open(data).unwrap().contents().unwrap();
    assert_eq!(&*contents.wasm[0].pipelines[0].1, b"p: 1");
}

#[test]
fn a_pipeline_whose_hash_does_not_match_is_refused() {
    let data = bundle(
        manifest(serde_json::json!({ "pipelines": [artifact("pipelines/a.yaml", b"a: 1")] })),
        &[("compute.wasm", b"job"), ("pipelines/a.yaml", b"a: 2")],
    );
    let err = open(data).unwrap().contents().err().unwrap();
    assert!(format!("{err:#}").contains("pipelines/a.yaml"), "{err:#}");
}

#[test]
fn a_pipeline_larger_than_its_declared_size_is_refused() {
    let big = vec![b'#'; 4096];
    let mut declared = artifact("pipelines/a.yaml", &big);
    declared["size"] = 10.into();
    let data = bundle(
        manifest(serde_json::json!({ "pipelines": [declared] })),
        &[("compute.wasm", b"job"), ("pipelines/a.yaml", &big)],
    );
    assert!(open(data).unwrap().contents().is_err());
}

#[test]
fn more_than_the_maximum_pipelines_are_refused() {
    let many = (0..=MAX_PIPELINES_PER_PACKAGE)
        .map(|i| artifact(&format!("p{i}.yaml"), b"x"))
        .collect::<Vec<_>>();
    let data = bundle(
        manifest(serde_json::json!({ "pipelines": many })),
        &[("compute.wasm", b"job")],
    );
    let err = open(data).err().unwrap();
    assert!(format!("{err:#}").contains("pipelines"), "{err:#}");
}
