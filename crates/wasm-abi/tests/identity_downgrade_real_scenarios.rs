//! End-to-end (in-process) proof of the identity-downgrade pipeline on the REAL
//! scenario shapes: embed -> read-back -> detect. No wasm build, no actor, no
//! network. The full actor/RPC refusal path is covered separately by the merobox
//! workflow `21-scenario-identity-downgrade`.
//!
//! The two schemas below are what `apps/migrations/scenario-identity-downgrade-v1`
//! and `-v2` build: one `wiki` field, `AuthoredMap` in v1 and `UnorderedMap` in
//! v2. They are spelled out here because this is a test of the downgrade logic,
//! not of how a manifest is produced.

use calimero_wasm_abi::downgrade::identity_downgrades;
use calimero_wasm_abi::embed::{
    read_embedded_state_schema, read_embedded_state_schema_versioned, write_embedded_state_schema,
    EmbeddedSchema,
};
use calimero_wasm_abi::schema::Manifest;
use serde_json::json;

fn empty_module() -> Vec<u8> {
    vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]
}

/// One `wiki` field of `map_kind`, keyed by string, holding an `LwwRegister<String>`.
fn wiki_schema(root: &str, version: u32, map_kind: &str) -> Manifest {
    serde_json::from_value(json!({
        "schema_version": "wasm-abi/1",
        "types": {
            root: {
                "kind": "record",
                "fields": [{
                    "name": "wiki",
                    "type": {
                        "kind": "map",
                        "key": { "kind": "string" },
                        "value": {
                            "kind": "record",
                            "fields": [],
                            "crdt_type": "lww_register",
                            "inner_type": { "kind": "string" },
                        },
                        "crdt_type": map_kind,
                    },
                }],
            },
        },
        "methods": [],
        "events": [],
        "state_root": root,
        "state_version": version,
    }))
    .expect("the scenario schema parses")
}

fn v1() -> Manifest {
    wiki_schema("ScenarioIdentityDowngradeV1", 1, "authored_map")
}

fn v2() -> Manifest {
    wiki_schema("ScenarioIdentityDowngradeV2", 2, "unordered_map")
}

/// Embed a schema into a minimal valid module, then read it back — exercising the
/// real wasm-section round-trip the node uses at upgrade time.
fn embed_then_read(schema: &Manifest) -> Manifest {
    let wasm = write_embedded_state_schema(&empty_module(), schema).expect("embed");
    read_embedded_state_schema(&wasm).expect("calimero_abi_v1 section present after embed")
}

#[test]
fn real_scenarios_round_trip_through_the_wasm_section() {
    // Sanity: the `wiki` field survives embed -> read.
    let v1 = embed_then_read(&v1());
    let root = v1.state_root.as_deref().expect("v1 has a state_root");
    let fields = match v1.types.get(root) {
        Some(calimero_wasm_abi::schema::TypeDef::Record { fields }) => fields,
        other => panic!("v1 state root is not a record: {other:?}"),
    };
    assert!(
        fields.iter().any(|f| f.name == "wiki"),
        "v1 has a `wiki` field"
    );
}

#[test]
fn real_v1_to_v2_is_flagged_as_identity_downgrade() {
    let v1 = embed_then_read(&v1());
    let v2 = embed_then_read(&v2());

    let downgrades = identity_downgrades(&v1, &v2);
    assert_eq!(downgrades.len(), 1, "exactly one downgrade: {downgrades:?}");
    assert_eq!(downgrades[0].field, "wiki");
    assert_eq!(downgrades[0].from, "AuthoredMap");
    assert_eq!(downgrades[0].to, "UnorderedMap");
}

#[test]
fn real_carry_through_is_not_a_downgrade() {
    let v1 = embed_then_read(&v1());
    assert!(
        identity_downgrades(&v1, &v1).is_empty(),
        "v1 -> v1 (carry-through) must not be flagged"
    );
}

/// A module whose embedded schema is from a NEWER toolchain (`wasm-abi/2`) must
/// read as present-but-opaque, not as "no schema". This is the property the
/// identity-downgrade gate relies on to fail *closed* instead of waving the
/// upgrade through. (The gate decision itself is unit-tested in
/// `calimero-context`; here we pin the wasm-section read that feeds it.)
#[test]
fn future_major_schema_reads_as_unsupported_not_absent() {
    let mut schema = v2();
    schema.schema_version = "wasm-abi/2".to_owned();
    let wasm = write_embedded_state_schema(&empty_module(), &schema).expect("embed");

    match read_embedded_state_schema_versioned(&wasm) {
        EmbeddedSchema::UnsupportedVersion(v) => assert_eq!(v, "wasm-abi/2"),
        other => panic!("expected UnsupportedVersion, got {other:?}"),
    }
    // The convenience Option reader still collapses it to None — exactly the
    // fail-open trap the gate avoids by using the versioned reader.
    assert!(read_embedded_state_schema(&wasm).is_none());
}
