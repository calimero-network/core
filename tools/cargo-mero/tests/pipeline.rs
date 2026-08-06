//! Slow integration test: builds the fixture app for real (needs network for
//! the git SDK deps on first run). Run in CI always; locally via `cargo test -- --ignored`.

use std::io::Read;
use std::path::Path;
use std::process::Command;

use flate2::read::GzDecoder;

/// A freshly scaffolded app must walk the whole new -> build -> test -> bundle
/// ladder cleanly.
///
/// `cargo mero new` pins `DEFAULT_SDK_VERSION`, a released tag whose SDK builds
/// no `__calimero_abi`. That is the degraded path: the wasm still builds, it
/// just carries no ABI, and `bundle` then has nothing to ship. The two
/// assertions at the end flip back once that pin names an SDK with the entry
/// point (see "Bumping the SDK version" in the README).
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
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "cargo mero build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let test = Command::new(bin)
        .args(["mero", "test", "--manifest-path"])
        .arg(app_dir.join("Cargo.toml"))
        .status()
        .unwrap();
    assert!(test.success(), "cargo mero test failed");

    // A build that can extract no ABI says so and writes none, rather than
    // shipping a wasm whose embedded section came from somewhere else.
    assert!(
        String::from_utf8_lossy(&build.stderr).contains("SDK provides no ABI entry point"),
        "the build must name the missing entry point:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    assert!(!app_dir.join("res/abi.json").exists());

    let bundle = Command::new(bin)
        .args(["mero", "bundle", "--dev", "--no-icon", "--manifest-path"])
        .arg(app_dir.join("Cargo.toml"))
        .output()
        .unwrap();
    assert!(
        !bundle.status.success(),
        "bundling an app with no ABI must fail"
    );
    assert!(
        String::from_utf8_lossy(&bundle.stderr).contains("cannot be bundled"),
        "bundle must say why:\n{}",
        String::from_utf8_lossy(&bundle.stderr)
    );
}

/// The `calimero_abi_v1` section read back off the built wasm - the only copy
/// the node consults when it resolves a migration plan.
fn embedded_abi(wasm_path: &Path) -> serde_json::Value {
    let wasm = std::fs::read(wasm_path).unwrap();
    let section = wasmparser::Parser::new(0)
        .parse_all(&wasm)
        .filter_map(Result::ok)
        .find_map(|p| match p {
            wasmparser::Payload::CustomSection(s) if s.name() == "calimero_abi_v1" => {
                Some(s.data().to_vec())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("{} must carry calimero_abi_v1", wasm_path.display()));
    serde_json::from_slice(&section).expect("calimero_abi_v1 section must be valid JSON")
}

/// The wasm's exported function names, i.e. what the bytecode actually offers.
fn wasm_exports(wasm_path: &Path) -> Vec<String> {
    let wasm = std::fs::read(wasm_path).unwrap();
    wasmparser::Parser::new(0)
        .parse_all(&wasm)
        .filter_map(Result::ok)
        .filter_map(|p| match p {
            wasmparser::Payload::ExportSection(s) => Some(s),
            _ => None,
        })
        .flat_map(|s| s.into_iter().filter_map(Result::ok))
        .map(|e| e.name.to_owned())
        .collect()
}

/// `--features` has to reach the compile AND the ABI extraction, or the bundle ships
/// bytecode and a `calimero_abi_v1` section that describe different schemas -
/// which surfaces as a silently wrong migration plan, not a build error.
#[test]
#[ignore = "slow: compiles the fixture services twice"]
fn features_select_the_same_schema_in_the_wasm_and_the_embedded_abi() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/multi-app");
    let build = |extra: &[&str]| {
        let status = Command::new(env!("CARGO_BIN_EXE_cargo-mero"))
            .args(["mero", "build"])
            .args(extra)
            .current_dir(&fixture)
            .status()
            .unwrap();
        assert!(status.success(), "cargo mero build {extra:?} failed");
    };
    let svc_a = fixture.join("crates/svc-a/res/svc_a.wasm");
    let svc_b = fixture.join("crates/svc-b/res/svc_b.wasm");

    build(&["--features", "schema_v2"]);
    assert_eq!(embedded_abi(&svc_a)["state_root"], "SvcAV2");
    assert!(
        wasm_exports(&svc_a).contains(&"revision".to_owned()),
        "the wasm itself must be the v2 build, not just its ABI"
    );
    // svc-b does not declare `schema_v2`, and must still be built and embedded.
    assert_eq!(embedded_abi(&svc_b)["state_root"], "SvcB");

    build(&[]);
    assert_eq!(embedded_abi(&svc_a)["state_root"], "SvcA");
    assert!(!wasm_exports(&svc_a).contains(&"revision".to_owned()));
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

    // A non-empty `methods` array proves the full ABI was embedded, and that
    // canonicalization ran: the fixture's methods are declared unsorted.
    let manifest = embedded_abi(&fixture.join("res/demo_app.wasm"));
    let methods = manifest["methods"]
        .as_array()
        .expect("embedded ABI must have a methods array");
    assert!(
        !methods.is_empty(),
        "embedded ABI must carry a non-empty methods array (full ABI, not state schema)"
    );
    assert!(fixture.join("res/abi.json").exists());

    let schema: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.join("res/state-schema.json")).unwrap())
            .unwrap();
    assert_eq!(
        schema["schema_version"], "wasm-abi/1",
        "the state schema must be stamped with the version the node reads"
    );
}

/// Multi-service bundling from a virtual workspace root with no --manifest-path,
/// covering cwd-based service discovery and the workspace-level app version.
#[test]
#[ignore = "slow: compiles two fixture service crates"]
fn multi_service_bundle() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/multi-app");
    let status = Command::new(env!("CARGO_BIN_EXE_cargo-mero"))
        .args(["mero", "bundle", "--dev", "--no-icon"])
        .current_dir(&fixture)
        .status()
        .unwrap();
    assert!(status.success(), "cargo mero bundle --dev failed");

    let mpk = fixture.join("dist/com.example.multi-app-0.1.0.mpk");
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
fn bundle_produces_signed_mpk() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/demo-app");
    let status = Command::new(env!("CARGO_BIN_EXE_cargo-mero"))
        .args(["mero", "bundle", "--dev", "--manifest-path"])
        .arg(fixture.join("Cargo.toml"))
        .status()
        .unwrap();
    assert!(status.success());

    // Default output: <base>/dist/<package>-<appVersion>.mpk, package and
    // version from the fixture's [package.metadata.calimero] table.
    let mpk = fixture.join("dist/com.example.demo-app-0.1.0.mpk");
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

    assert_eq!(
        manifest["handlers"]["slug"], "com.example.demo-app",
        "slug must default to the package id"
    );
    assert!(
        manifest["metadata"]["icon"]
            .as_str()
            .is_some_and(|icon| icon.starts_with("data:image/png;base64,")),
        "icon must be encoded as a PNG data URI: {}",
        manifest["metadata"]["icon"]
    );
    assert_no_nulls(&manifest, "$");
}
