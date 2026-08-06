use calimero_wasm_abi::abi_type::{AbiType, TypeRegistry};
use calimero_wasm_abi::manifest_builder::ManifestBuilder;
use calimero_wasm_abi::schema::{Method, MethodIntent, TypeRef};
use calimero_wasm_abi::validate::validate_manifest;

fn method(name: &str) -> Method {
    Method {
        name: name.to_owned(),
        params: vec![],
        returns: None,
        returns_nullable: None,
        errors: vec![],
        intent: MethodIntent::Unspecified,
        xcall_callable: false,
        xcall_callers: Default::default(),
    }
}

fn event(name: &str) -> calimero_wasm_abi::schema::Event {
    calimero_wasm_abi::schema::Event {
        name: name.to_owned(),
        payload: None,
    }
}

#[test]
fn finish_sorts_methods_and_events_by_name_and_validates() {
    let mut b = ManifestBuilder::new();
    b.method(method("zeta"));
    b.method(method("alpha"));
    b.method(method("mid"));
    b.event(event("zulu"));
    b.event(event("alpha"));

    let manifest = b.finish();

    assert_eq!(
        manifest
            .methods
            .iter()
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha", "mid", "zeta"]
    );
    assert_eq!(
        manifest
            .events
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha", "zulu"]
    );
    assert!(validate_manifest(&manifest).is_ok());
}

// Two entries that sort to the same key keep their push order: validate_manifest
// accepts only this ordering, so the sort here is the one that has to be stable.
#[test]
fn finish_sort_is_stable_for_equal_names() {
    let mut b = ManifestBuilder::new();
    let mut first = method("dup");
    first.returns = Some(TypeRef::string());
    let mut second = method("dup");
    second.returns = Some(TypeRef::i32());
    b.method(first.clone());
    b.method(second.clone());

    let manifest = b.finish();

    assert_eq!(manifest.methods[0].returns, first.returns);
    assert_eq!(manifest.methods[1].returns, second.returns);
}

#[test]
fn ref_and_ref_mut_type_ref_equal_the_owned_type() {
    let mut reg = TypeRegistry::new();

    assert_eq!(
        <&String as AbiType>::type_ref(&mut reg),
        <String as AbiType>::type_ref(&mut reg)
    );
    assert_eq!(<&str as AbiType>::type_ref(&mut reg), TypeRef::string());
    assert_eq!(
        <&mut String as AbiType>::type_ref(&mut reg),
        <String as AbiType>::type_ref(&mut reg)
    );
}

#[test]
fn state_root_version_and_migration_land_in_manifest_fields() {
    let mut b = ManifestBuilder::new();
    b.state_root("MyState");
    b.state_version(3);
    b.migration("migrate_v1_to_v2", 1);

    let manifest = b.finish();

    assert_eq!(manifest.state_root, Some("MyState".to_owned()));
    assert_eq!(manifest.state_version, Some(3));
    assert_eq!(manifest.migrations.len(), 1);
    assert_eq!(manifest.migrations[0].method, "migrate_v1_to_v2");
    assert_eq!(manifest.migrations[0].from_version, 1);
}

// Absent-key semantics come from the schema's serde attrs; the builder must
// not force these keys to appear when nothing was ever set.
#[test]
fn unset_optional_fields_serialize_with_absent_keys() {
    let manifest = ManifestBuilder::new().finish();
    let json = serde_json::to_value(&manifest).unwrap();

    assert!(json.get("state_root").is_none(), "{json}");
    assert!(json.get("state_version").is_none(), "{json}");
    assert!(json.get("migrations").is_none(), "{json}");
}

#[test]
fn set_optional_fields_serialize_with_present_keys() {
    let mut b = ManifestBuilder::new();
    b.state_root("MyState");
    b.state_version(3);
    b.migration("migrate_v1_to_v2", 1);
    let json = serde_json::to_value(b.finish()).unwrap();

    assert_eq!(json["state_root"], "MyState");
    assert_eq!(json["state_version"], 3);
    assert_eq!(json["migrations"][0]["method"], "migrate_v1_to_v2");
    assert_eq!(json["migrations"][0]["fromVersion"], 1);
}
