use std::collections::BTreeSet;
use std::path::Path;

use calimero_app_build::{abi_of, build_component, imports_of};
use calimero_app_wit::{allowed_imports, stubbed_imports};

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
        r#"{"world":"calimero:app@0.5.0","methods":{"echo":{"kind":"call"}}}"#,
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
