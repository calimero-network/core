use std::collections::BTreeSet;
use std::path::Path;

use calimero_app_build::{
    abi_of, build_component, build_components, build_components_cached, imports_of,
};
use calimero_app_wit::{allowed_imports, compute_allowed_imports, stubbed_imports, COMPUTE_WORLD};

#[test]
fn echo_guest_builds_into_a_component_with_only_world_imports() {
    let target = tempfile::tempdir().expect("tempdir");
    let guest = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/guests/echo");
    let built = build_component(&guest, target.path()).expect("build");

    let known: BTreeSet<String> = allowed_imports()
        .iter()
        .chain(stubbed_imports())
        .map(|s| (*s).to_owned())
        .collect();
    assert!(!built.imports.is_empty());
    for import in &built.imports {
        assert!(known.contains(import), "unexpected import {import}");
        assert!(!import.starts_with("wasi:sockets"), "{import}");
    }
    assert_eq!(built.imports, imports_of(&built.component).unwrap());

    let expected: serde_json::Value = serde_json::from_str(
        r#"{"world":"calimero:app@0.5.1","methods":{"echo":{"kind":"call"}}}"#,
    )
    .unwrap();
    assert_eq!(built.abi, Some(expected.clone()));
    assert_eq!(abi_of(&built.component).unwrap(), Some(expected));
}

/// The cluster fixtures (`apps/v2-fixtures/*`) build into components that import only the
/// world's interfaces (allowed or stubbed), and carry an ABI.
#[test]
fn fixtures_build_with_world_imports_only() {
    let known: BTreeSet<String> = allowed_imports()
        .iter()
        .chain(stubbed_imports())
        .map(|s| (*s).to_owned())
        .collect();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/v2-fixtures");
    for name in ["kv", "org-gated"] {
        let target = tempfile::tempdir().expect("tempdir");
        let built = build_component(&root.join(name), target.path())
            .unwrap_or_else(|e| panic!("build {name}: {e}"));
        assert!(!built.imports.is_empty(), "{name}");
        for import in &built.imports {
            // Rust's std imports the WASI patch it was built with; the engine links any
            // older 0.2 patch (D43), so compare with the patch pinned.
            let pinned = match import.strip_prefix("wasi:") {
                Some(rest) => match rest.rsplit_once("@0.2.") {
                    Some((iface, _)) => format!("wasi:{iface}@0.2.12"),
                    None => import.clone(),
                },
                None => import.clone(),
            };
            assert!(
                known.contains(&pinned),
                "{name}: unexpected import {import}"
            );
            assert!(!import.starts_with("wasi:sockets"), "{name}: {import}");
        }
        assert!(built.abi.is_some(), "{name}: no ABI");
    }
}

fn pinned(import: &str) -> String {
    // Rust's std imports the WASI patch it was built with; the engine links any older 0.2 patch.
    match import
        .strip_prefix("wasi:")
        .and_then(|r| r.rsplit_once("@0.2."))
    {
        Some((iface, _)) => format!("wasi:{iface}@0.2.12"),
        None => import.to_owned(),
    }
}

#[test]
fn a_crate_with_compute_methods_builds_a_second_component_that_cannot_reach_state_or_http() {
    let target = tempfile::tempdir().expect("tempdir");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/v2-fixtures/compute");
    let built = build_components(&fixture, target.path()).expect("build");
    let compute = built.compute.expect("a compute component");

    let known: BTreeSet<String> = compute_allowed_imports()
        .iter()
        .chain(stubbed_imports())
        .map(|s| (*s).to_owned())
        .collect();
    assert!(!compute.imports.is_empty());
    for import in &compute.imports {
        assert!(
            known.contains(&pinned(import)),
            "unexpected import {import}"
        );
    }
    let abi = compute.abi.expect("a compute ABI");
    assert_eq!(abi["world"], COMPUTE_WORLD);
    let methods = abi["methods"].as_object().expect("methods");
    assert_eq!(
        methods.keys().collect::<Vec<_>>(),
        [
            "big",
            "burn",
            "digest",
            "digestThen",
            "digestUnkept",
            "hog",
            "peek",
            "spin"
        ]
    );
    assert!(methods.values().all(|m| m["kind"] == "compute"));

    let app = built.app;
    assert_ne!(app.component, compute.component);
    assert!(app
        .imports
        .iter()
        .any(|i| i.starts_with("calimero:app/storage")));
    assert!(app
        .imports
        .iter()
        .all(|i| !i.starts_with("calimero:app/job")));
    assert_eq!(app.abi.expect("an app ABI")["world"], "calimero:app@0.5.1");
}

#[test]
fn a_crate_without_compute_methods_builds_one_component() {
    let target = tempfile::tempdir().expect("tempdir");
    let guest = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/guests/echo");
    let built = build_components(&guest, target.path()).expect("build");
    assert!(built.compute.is_none());
    assert!(!built.app.component.is_empty());
}

#[test]
fn the_cli_writes_the_compute_component_beside_the_app_component() {
    let out = tempfile::tempdir().expect("tempdir");
    let target = tempfile::tempdir().expect("tempdir");
    let app = out.path().join("fixture.wasm");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/v2-fixtures/compute");
    let run = std::process::Command::new(env!("CARGO_BIN_EXE_mero-app-build"))
        .arg(&fixture)
        .arg("--target-dir")
        .arg(target.path())
        .arg("--out")
        .arg(&app)
        .output()
        .expect("run mero-app-build");
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(app.exists());
    let compute = std::fs::read(out.path().join("compute.component.wasm")).expect("compute out");
    let abi = abi_of(&compute).unwrap().expect("a compute ABI");
    assert_eq!(abi["world"], COMPUTE_WORLD);

    let echo = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/guests/echo");
    let refused = std::process::Command::new(env!("CARGO_BIN_EXE_mero-app-build"))
        .arg(&echo)
        .arg("--target-dir")
        .arg(target.path())
        .arg("--out")
        .arg(out.path().join("echo.wasm"))
        .arg("--compute-out")
        .arg(out.path().join("echo-compute.wasm"))
        .output()
        .expect("run mero-app-build");
    assert!(!refused.status.success());
}

#[test]
fn the_cache_keeps_both_components_and_whether_there_is_a_compute_one() {
    let cache = tempfile::tempdir().expect("tempdir");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let sdk = root.join("crates/sdk");
    let macros = root.join("crates/sdk-macros");
    let wit = root.join("crates/app-wit");
    let also = [sdk.as_path(), macros.as_path(), wit.as_path()];

    let fixture = root.join("apps/v2-fixtures/compute");
    let first = build_components_cached(&fixture, &also, cache.path()).expect("build");
    let compute = first.compute.clone().expect("a compute component");
    assert_ne!(first.app, compute);
    let again = build_components_cached(&fixture, &also, cache.path()).expect("cached");
    assert_eq!((again.app, again.compute), (first.app, Some(compute)));

    let echo = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/guests/echo");
    let plain = build_components_cached(&echo, &[], cache.path()).expect("build");
    assert!(plain.compute.is_none());
    let plain_again = build_components_cached(&echo, &[], cache.path()).expect("cached");
    assert!(plain_again.compute.is_none());
}

#[test]
fn a_crate_with_a_cdylib_dependency_packages_its_own_component() {
    let target = tempfile::tempdir().expect("tempdir");
    let top = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/guests/chain/top");
    let built = build_component(&top, target.path()).expect("build");
    let abi = built.abi.expect("an ABI");
    let methods = abi["methods"].as_object().expect("methods");
    assert_eq!(methods.keys().collect::<Vec<_>>(), ["top_only"]);
}
