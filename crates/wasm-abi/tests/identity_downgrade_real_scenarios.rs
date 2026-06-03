//! End-to-end (in-process) proof of the identity-downgrade pipeline on the REAL
//! scenario apps: real emitter -> embed -> read-back -> detect. No wasm build,
//! no actor, no network. The full actor/RPC refusal path is covered separately
//! by the merobox workflow `21-scenario-identity-downgrade`.

use calimero_wasm_abi::downgrade::identity_downgrades;
use calimero_wasm_abi::embed::{read_embedded_state_schema, write_embedded_state_schema};
use calimero_wasm_abi::emitter::emit_manifest;
use calimero_wasm_abi::schema::Manifest;

const V1_SRC: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../apps/migrations/scenario-identity-downgrade-v1/src/lib.rs"
));
const V2_SRC: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../apps/migrations/scenario-identity-downgrade-v2/src/lib.rs"
));

fn state_schema(src: &str) -> Manifest {
    let manifest = emit_manifest(src).expect("emit_manifest on scenario source");
    manifest
        .extract_state_schema()
        .expect("extract_state_schema")
}

/// Embed a schema into a minimal valid module, then read it back — exercising the
/// real wasm-section round-trip the node uses at upgrade time.
fn embed_then_read(schema: &Manifest) -> Manifest {
    let empty_module = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
    let wasm = write_embedded_state_schema(&empty_module, schema).expect("embed");
    read_embedded_state_schema(&wasm).expect("calimero_abi_v1 section present after embed")
}

#[test]
fn real_scenarios_round_trip_through_the_wasm_section() {
    // Sanity: the real emitter produces a state schema with the `wiki` field,
    // and it survives embed -> read.
    let v1 = embed_then_read(&state_schema(V1_SRC));
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
    let v1 = embed_then_read(&state_schema(V1_SRC));
    let v2 = embed_then_read(&state_schema(V2_SRC));

    let downgrades = identity_downgrades(&v1, &v2);
    assert_eq!(downgrades.len(), 1, "exactly one downgrade: {downgrades:?}");
    assert_eq!(downgrades[0].field, "wiki");
    assert_eq!(downgrades[0].from, "AuthoredMap");
    assert_eq!(downgrades[0].to, "UnorderedMap");
}

#[test]
fn real_carry_through_is_not_a_downgrade() {
    let v1 = embed_then_read(&state_schema(V1_SRC));
    assert!(
        identity_downgrades(&v1, &v1).is_empty(),
        "v1 -> v1 (carry-through) must not be flagged"
    );
}
