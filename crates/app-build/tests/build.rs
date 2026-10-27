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
