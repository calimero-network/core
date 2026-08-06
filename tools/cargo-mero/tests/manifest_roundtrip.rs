//! Proves `manifest::render` output round-trips through the node's own
//! `BundleManifest`, so a dropped field fails here, not on the registry page.

use calimero_bundle::BundleManifest;
use camino::Utf8PathBuf;
use cargo_mero::manifest::{artifact_from_bytes, StagedArtifact};
use cargo_mero::meta::BundleMeta;

fn full_meta() -> BundleMeta {
    BundleMeta {
        package: "com.example.demo".into(),
        name: Some("Demo".into()),
        description: Some("A demo app".into()),
        author: Some("Acme".into()),
        icon: Some("data:image/png;base64,iVBORw0KGgo=".into()),
        slug: Some("com.example.demo".into()),
        license: Some("MIT".into()),
        tags: vec!["social".into()],
        github: Some("https://github.com/acme/demo".into()),
        docs: Some("https://docs.acme.com".into()),
        min_runtime_version: "0.1.0".into(),
        frontend: Some("https://example.com".into()),
        app_version: "1.0.0".into(),
        services: vec![],
        manifest_dir: Utf8PathBuf::new(),
    }
}

fn staged() -> Vec<StagedArtifact> {
    vec![StagedArtifact {
        service_name: None,
        wasm: artifact_from_bytes("app.wasm", b"wasm-bytes"),
        abi: Some(artifact_from_bytes("abi.json", b"abi-bytes")),
    }]
}

fn staged_without_abi() -> Vec<StagedArtifact> {
    vec![StagedArtifact {
        service_name: None,
        wasm: artifact_from_bytes("app.wasm", b"wasm-bytes"),
        abi: None,
    }]
}

fn sparse_meta() -> BundleMeta {
    BundleMeta {
        package: "com.example.demo".into(),
        name: Some("Demo".into()),
        description: None,
        author: None,
        icon: None,
        slug: None,
        license: None,
        tags: vec![],
        github: None,
        docs: None,
        min_runtime_version: "0.1.0".into(),
        frontend: None,
        app_version: "1.0.0".into(),
        services: vec![],
        manifest_dir: Utf8PathBuf::new(),
    }
}

/// Fails if `value` contains a `null` anywhere: a registry validator that
/// tolerates an absent field still rejects one present as explicit `null`.
fn assert_no_nulls(value: &serde_json::Value, path: &str) {
    match value {
        serde_json::Value::Null => panic!("unexpected null at {path}"),
        serde_json::Value::Object(map) => {
            for (key, v) in map {
                assert_no_nulls(v, &format!("{path}.{key}"));
            }
        }
        serde_json::Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                assert_no_nulls(v, &format!("{path}[{i}]"));
            }
        }
        _ => {}
    }
}

#[test]
fn omitted_optional_fields_serialize_without_nulls() {
    let json = cargo_mero::manifest::render(&sparse_meta(), &staged()).expect("render");
    assert_no_nulls(&json, "$");

    // Still deserializes back into the node's type with the sparse fields
    // reading as `None`, proving skip-on-serialize hasn't relaxed parsing.
    let m: BundleManifest = serde_json::from_value(json).expect("node accepts it");
    let meta = m.metadata.expect("metadata");
    assert!(meta.author.is_none());
    assert!(meta.license.is_none());
}

#[test]
fn every_field_survives_a_round_trip_through_the_node_type() {
    let json = cargo_mero::manifest::render(&full_meta(), &staged()).expect("render");
    let m: BundleManifest = serde_json::from_value(json).expect("node accepts it");

    let meta = m.metadata.expect("metadata");
    assert_eq!(meta.name, "Demo");
    assert_eq!(meta.author.as_deref(), Some("Acme"));
    assert_eq!(meta.license.as_deref(), Some("MIT"));
    assert_eq!(meta.tags, vec!["social".to_owned()]);
    assert!(meta
        .icon
        .expect("icon")
        .starts_with("data:image/png;base64,"));

    assert_eq!(
        m.handlers.expect("handlers").slug.as_deref(),
        Some("com.example.demo")
    );
    let links = m.links.expect("links");
    assert_eq!(
        links.github.as_deref(),
        Some("https://github.com/acme/demo")
    );
    assert_eq!(links.docs.as_deref(), Some("https://docs.acme.com"));
}

/// Guards backward compatibility: a bundle published before `skip_serializing_if`
/// wrote these fields as explicit JSON `null`, and must still install today.
#[test]
fn old_manifest_with_explicit_nulls_still_parses() {
    let json = r#"{
        "version": "1.0",
        "package": "com.example.demo",
        "appVersion": "1.0.0",
        "minRuntimeVersion": "0.1.0",
        "metadata": { "name": "Demo", "description": null, "author": null, "icon": null, "license": null },
        "links": { "frontend": null, "github": null, "docs": null },
        "wasm": { "path": "app.wasm", "hash": "00", "size": 10 }
    }"#;

    let m: BundleManifest = serde_json::from_str(json).expect("old manifest still parses");

    let meta = m.metadata.expect("metadata");
    assert!(meta.description.is_none());
    assert!(meta.author.is_none());
    assert!(meta.icon.is_none());
    assert!(meta.license.is_none());

    let links = m.links.expect("links");
    assert!(links.frontend.is_none());
    assert!(links.github.is_none());
    assert!(links.docs.is_none());
}

#[test]
fn no_abi_omits_the_artifact_entirely() {
    let json = cargo_mero::manifest::render(&full_meta(), &staged_without_abi()).expect("render");
    assert!(
        json.get("abi").is_none(),
        "an omitted abi must not serialize as null"
    );
    let m: BundleManifest = serde_json::from_value(json).expect("node accepts it");
    assert!(m.abi.is_none());
}
