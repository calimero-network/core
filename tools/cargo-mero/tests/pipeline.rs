//! Slow integration test: builds the fixture app for real (needs network for
//! the git SDK deps on first run). Run in CI always; locally via `cargo test -- --ignored`.

use std::io::Read;
use std::path::Path;
use std::process::Command;

use flate2::read::GzDecoder;

/// A freshly scaffolded app must walk the whole new -> build -> test -> bundle
/// ladder cleanly.
#[test]
#[ignore = "slow: scaffolds and compiles a fresh app (needs network for git SDK deps)"]
fn new_build_test_bundle_ladder() {
    let tmp = tempfile::tempdir().unwrap();
    let app_dir = tmp.path().join("ladder-app");
    let bin = env!("CARGO_BIN_EXE_cargo-mero");

    let new = Command::new(bin)
        .args(["mero", "new", "ladder-app", "--path"])
        .arg(&app_dir)
        .status()
        .unwrap();
    assert!(new.success(), "cargo mero new failed");

    let build = Command::new(bin)
        .args(["mero", "build", "--manifest-path"])
        .arg(app_dir.join("Cargo.toml"))
        .status()
        .unwrap();
    assert!(build.success(), "cargo mero build failed");

    let test = Command::new(bin)
        .args(["mero", "test", "--manifest-path"])
        .arg(app_dir.join("Cargo.toml"))
        .status()
        .unwrap();
    assert!(test.success(), "cargo mero test failed");

    let bundle = Command::new(bin)
        .args(["mero", "bundle", "--dev", "--manifest-path"])
        .arg(app_dir.join("Cargo.toml"))
        .status()
        .unwrap();
    assert!(bundle.success(), "cargo mero bundle failed");

    let mpk = app_dir.join("dist/com.example.ladder-app.mpk");
    assert!(mpk.exists(), "expected bundle at {}", mpk.display());
}

#[test]
#[ignore = "slow: compiles the fixture app"]
fn build_produces_embedded_abi_wasm() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/demo-app");
    let status = Command::new(env!("CARGO_BIN_EXE_cargo-mero"))
        .args(["mero", "build", "--manifest-path"])
        .arg(fixture.join("Cargo.toml"))
        .status()
        .unwrap();
    assert!(status.success());

    let wasm = std::fs::read(fixture.join("res/demo_app.wasm")).unwrap();
    let section = wasmparser::Parser::new(0)
        .parse_all(&wasm)
        .filter_map(Result::ok)
        .find_map(|p| match p {
            wasmparser::Payload::CustomSection(s) if s.name() == "calimero_abi_v1" => {
                Some(s.data().to_vec())
            }
            _ => None,
        })
        .expect("built wasm must carry calimero_abi_v1");

    // A non-empty `methods` array proves the full ABI was embedded, and that
    // canonicalization ran: the fixture's methods are declared unsorted.
    let manifest: serde_json::Value =
        serde_json::from_slice(&section).expect("calimero_abi_v1 section must be valid JSON");
    let methods = manifest["methods"]
        .as_array()
        .expect("embedded ABI must have a methods array");
    assert!(
        !methods.is_empty(),
        "embedded ABI must carry a non-empty methods array (full ABI, not state schema)"
    );
    assert!(fixture.join("res/abi.json").exists());
}

/// Multi-service bundling from a virtual workspace root with no --manifest-path,
/// covering cwd-based service discovery and the workspace-level app version.
#[test]
#[ignore = "slow: compiles two fixture service crates"]
fn multi_service_bundle() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/multi-app");
    let status = Command::new(env!("CARGO_BIN_EXE_cargo-mero"))
        .args(["mero", "bundle", "--dev"])
        .current_dir(&fixture)
        .status()
        .unwrap();
    assert!(status.success(), "cargo mero bundle --dev failed");

    let mpk = fixture.join("dist/com.example.multi-app.mpk");
    assert!(mpk.exists(), "expected bundle at {}", mpk.display());

    let mut entries = Vec::new();
    let mut manifest_bytes = Vec::new();
    let mut archive = tar::Archive::new(GzDecoder::new(std::fs::File::open(&mpk).unwrap()));
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = entry.path().unwrap().to_string_lossy().into_owned();
        if path == "manifest.json" {
            entry.read_to_end(&mut manifest_bytes).unwrap();
        }
        entries.push(path);
    }
    entries.sort();
    assert_eq!(
        entries,
        vec![
            "manifest.json",
            "services/svc-a-abi.json",
            "services/svc-a.wasm",
            "services/svc-b-abi.json",
            "services/svc-b.wasm",
        ]
    );

    let manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
    assert_eq!(manifest["services"].as_array().unwrap().len(), 2);
    assert_eq!(manifest["appVersion"], "0.1.0");
    assert_eq!(manifest["signature"]["algorithm"], "ed25519");
    assert!(
        mero_sign::verify_manifest(&manifest).unwrap(),
        "signature must verify against the embedded public key"
    );
}

#[test]
#[ignore = "slow: compiles the fixture app"]
fn bundle_produces_signed_mpk() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/demo-app");
    let status = Command::new(env!("CARGO_BIN_EXE_cargo-mero"))
        .args(["mero", "bundle", "--dev", "--manifest-path"])
        .arg(fixture.join("Cargo.toml"))
        .status()
        .unwrap();
    assert!(status.success());

    // Default output: <base>/dist/<package>.mpk, package from the fixture's
    // [package.metadata.calimero] table.
    let mpk = fixture.join("dist/com.example.demo-app.mpk");
    assert!(mpk.exists(), "expected bundle at {}", mpk.display());

    let mut entries = Vec::new();
    let mut manifest_bytes = Vec::new();
    let mut archive = tar::Archive::new(GzDecoder::new(std::fs::File::open(&mpk).unwrap()));
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = entry.path().unwrap().to_string_lossy().into_owned();
        if path == "manifest.json" {
            entry.read_to_end(&mut manifest_bytes).unwrap();
        }
        entries.push(path);
    }
    entries.sort();
    assert_eq!(entries, vec!["abi.json", "app.wasm", "manifest.json"]);

    let manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
    assert_eq!(manifest["signature"]["algorithm"], "ed25519");
    assert_eq!(manifest["signerId"], mero_sign::dev_signer_id());
    assert!(
        mero_sign::verify_manifest(&manifest).unwrap(),
        "signature must verify against the embedded public key"
    );
}
