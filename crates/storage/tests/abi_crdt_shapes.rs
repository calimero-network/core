//! Locks the exact ABI shape of every CRDT wrapper against what the syn
//! normalizer produces today. `inner_type` is populated ONLY where
//! `CollectionType` cannot carry the payload - two wrappers out of thirteen -
//! and CRDT maps always describe their key as `string` no matter the Rust
//! key type. A uniform-looking refactor of these impls is a bug, and this
//! file is what catches it.

use calimero_storage::collections::{
    AccessControl, AuthoredMap, AuthoredVector, BlockView, Counter, DefaultMarks, FrozenStorage,
    FrozenValue, FugueText, GCounter, LwwRegister, Ownable, PNCounter, ReplicatedGrowableArray,
    RichDocument, RichText, SharedStorage, SortedMap, SortedSet, Span, UnorderedMap, UnorderedSet,
    UserStorage, Vector, WriterSetCell,
};
use calimero_wasm_abi::abi_type::{AbiType, TypeRegistry};
use calimero_wasm_abi::schema::{CollectionType, CrdtCollectionType, ScalarType, TypeDef, TypeRef};

fn ref_of<T: AbiType>() -> TypeRef {
    let mut reg = TypeRegistry::new();
    <T as AbiType>::type_ref(&mut reg)
}

fn parts(
    r: TypeRef,
) -> (
    CollectionType,
    Option<CrdtCollectionType>,
    Option<Box<TypeRef>>,
) {
    match r {
        TypeRef::Collection {
            collection,
            crdt_type,
            inner_type,
        } => (collection, crdt_type, inner_type),
        other => panic!("expected a collection, got {other:?}"),
    }
}

const STR: TypeRef = TypeRef::Scalar(ScalarType::String);

// ── payload in List.items, inner_type None ──────────────────────────────

#[test]
fn vector_is_a_list_with_no_inner_type() {
    let (c, crdt, inner) = parts(ref_of::<Vector<String>>());
    assert_eq!(crdt, Some(CrdtCollectionType::Vector));
    assert_eq!(inner, None, "Vector puts its payload in List.items");
    let CollectionType::List { items } = c else {
        panic!("expected list")
    };
    assert_eq!(*items, STR);
}

#[test]
fn authored_vector_is_a_list_with_no_inner_type() {
    let (c, crdt, inner) = parts(ref_of::<AuthoredVector<String>>());
    assert_eq!(crdt, Some(CrdtCollectionType::AuthoredVector));
    assert_eq!(inner, None);
    assert!(matches!(c, CollectionType::List { .. }));
}

#[test]
fn unordered_set_is_a_list_with_no_inner_type() {
    let (c, crdt, inner) = parts(ref_of::<UnorderedSet<String>>());
    assert_eq!(crdt, Some(CrdtCollectionType::UnorderedSet));
    assert_eq!(inner, None);
    assert!(matches!(c, CollectionType::List { .. }));
}

#[test]
fn sorted_set_is_a_list_with_no_inner_type() {
    let (c, crdt, inner) = parts(ref_of::<SortedSet<String>>());
    assert_eq!(crdt, Some(CrdtCollectionType::SortedSet));
    assert_eq!(inner, None);
    assert!(matches!(c, CollectionType::List { .. }));
}

// ── payload in Map.key/value, inner_type None ───────────────────────────

#[test]
fn unordered_map_is_a_map_with_no_inner_type() {
    let (c, crdt, inner) = parts(ref_of::<UnorderedMap<String, u64>>());
    assert_eq!(crdt, Some(CrdtCollectionType::UnorderedMap));
    assert_eq!(
        inner, None,
        "UnorderedMap puts its payload in Map.key/value"
    );
    let CollectionType::Map { key, value } = c else {
        panic!("expected map")
    };
    assert_eq!(*key, STR);
    assert_eq!(*value, TypeRef::Scalar(ScalarType::U64));
}

#[test]
fn sorted_map_is_a_map_with_no_inner_type() {
    let (c, crdt, inner) = parts(ref_of::<SortedMap<String, u64>>());
    assert_eq!(crdt, Some(CrdtCollectionType::SortedMap));
    assert_eq!(inner, None);
    assert!(matches!(c, CollectionType::Map { .. }));
}

#[test]
fn authored_map_is_a_map_with_no_inner_type() {
    let (c, crdt, inner) = parts(ref_of::<AuthoredMap<String, u64>>());
    assert_eq!(crdt, Some(CrdtCollectionType::AuthoredMap));
    assert_eq!(inner, None);
    assert!(matches!(c, CollectionType::Map { .. }));
}

#[test]
fn crdt_map_key_is_string_regardless_of_rust_key_type() {
    // The CRDT layer keys entries internally; the normalizer always emits
    // `string` for the key, so the impls must too.
    let (c, crdt, inner) = parts(ref_of::<UnorderedMap<u64, String>>());
    assert_eq!(crdt, Some(CrdtCollectionType::UnorderedMap));
    assert_eq!(inner, None);
    let CollectionType::Map { key, value } = c else {
        panic!("expected map")
    };
    assert_eq!(*key, STR, "a u64 Rust key still describes as string");
    assert_eq!(*value, STR);
}

// ── the two exceptions: empty record placeholder + inner_type Some ──────

#[test]
fn lww_register_is_an_empty_record_carrying_inner_type() {
    let (c, crdt, inner) = parts(ref_of::<LwwRegister<String>>());
    assert_eq!(crdt, Some(CrdtCollectionType::LwwRegister));
    let CollectionType::Record { fields } = c else {
        panic!("expected record")
    };
    assert!(
        fields.is_empty(),
        "the record is a placeholder; the type is in inner_type"
    );
    assert_eq!(
        inner.map(|b| *b),
        Some(STR),
        "a consumer reads inner_type to deserialize (value, timestamp, node_id)"
    );
}

#[test]
fn shared_storage_is_an_empty_record_carrying_inner_type() {
    let (c, crdt, inner) = parts(ref_of::<SharedStorage<LwwRegister<String>>>());
    assert_eq!(crdt, Some(CrdtCollectionType::SharedStorage));
    let CollectionType::Record { fields } = c else {
        panic!("expected record")
    };
    assert!(fields.is_empty());
    assert_eq!(
        inner.map(|b| *b),
        Some(ref_of::<LwwRegister<String>>()),
        "inner_type carries the guarded value's own shape"
    );
}

#[test]
fn access_control_is_a_guarded_role_map() {
    let (c, crdt, inner) = parts(ref_of::<AccessControl>());
    assert_eq!(
        crdt,
        Some(CrdtCollectionType::SharedStorage),
        "the writer-set ACL must stay visible"
    );
    assert_eq!(inner, None);
    let CollectionType::Map { key, value } = c else {
        panic!("expected map")
    };
    assert_eq!(*key, STR);
    assert_eq!(*value, TypeRef::Scalar(ScalarType::Bool));
}

#[test]
fn ownable_shares_the_shared_storage_shape() {
    // Ownable<T> is PermissionedStorage under a different policy; the policy
    // is a zero-sized marker, so the ABI shape is SharedStorage's.
    assert_eq!(
        ref_of::<Ownable<LwwRegister<String>>>(),
        ref_of::<SharedStorage<LwwRegister<String>>>()
    );
}

// ── per-identity storage: a plain map, no crdt_type ─────────────────────

#[test]
fn per_identity_storage_is_a_plain_map() {
    // `UserStorage`/`FrozenStorage` partition by identity instead of merging,
    // so the ABI carries no CRDT tag - the marker a consumer keys convergence
    // behaviour off.
    for r in [ref_of::<UserStorage<u64>>(), ref_of::<FrozenStorage<u64>>()] {
        let (c, crdt, inner) = parts(r);
        assert_eq!(crdt, None, "not a CRDT");
        assert_eq!(inner, None, "the payload rides Map.value");
        let CollectionType::Map { key, value } = c else {
            panic!("expected map")
        };
        assert_eq!(*key, STR, "the identity key describes as a string");
        assert_eq!(*value, TypeRef::Scalar(ScalarType::U64));
    }
}

/// A composite of existing collections has no tag of its own, and what a client
/// receives is the RENDERED span rather than the stored mark row.
#[test]
fn rich_text_is_an_untagged_map_of_rendered_spans() {
    let mut reg = TypeRegistry::new();
    let (c, crdt, inner) = parts(<RichText<DefaultMarks> as AbiType>::type_ref(&mut reg));
    assert_eq!(crdt, None, "a pure composite carries no CRDT tag");
    assert_eq!(inner, None, "the payload rides Map.value");
    let CollectionType::Map { key, value } = c else {
        panic!("expected map")
    };
    assert_eq!(*key, STR);
    assert_eq!(*value, TypeRef::reference("Span"));

    <Span as AbiType>::register(&mut reg);
    let TypeDef::Record { fields } = reg
        .into_types()
        .remove("Span")
        .expect("a span must be a named type")
    else {
        panic!("expected a record")
    };
    let names: Vec<&str> = fields.iter().map(|field| field.name.as_str()).collect();
    assert_eq!(names, ["text", "attributes"]);
}

/// The block list is the same untagged shape, and what it advertises is the
/// rendered block rather than the stored row.
#[test]
fn rich_document_is_an_untagged_map_of_rendered_blocks() {
    let mut reg = TypeRegistry::new();
    let (c, crdt, inner) = parts(<RichDocument<DefaultMarks> as AbiType>::type_ref(&mut reg));
    assert_eq!(crdt, None, "a pure composite carries no CRDT tag");
    assert_eq!(inner, None, "the payload rides Map.value");
    let CollectionType::Map { key, value } = c else {
        panic!("expected map")
    };
    assert_eq!(*key, STR);
    assert_eq!(*value, TypeRef::reference("BlockView"));

    <BlockView as AbiType>::register(&mut reg);
    let types = reg.into_types();
    let TypeDef::Record { fields } = types
        .get("BlockView")
        .expect("a block view must be a named type")
        .clone()
    else {
        panic!("expected a record")
    };
    let names: Vec<&str> = fields.iter().map(|field| field.name.as_str()).collect();
    assert_eq!(names, ["id", "kind", "depth", "attrs", "spans"]);
    assert!(
        types.contains_key("Span"),
        "a block view's spans must pull the span record in with it"
    );
}

// ── opaque: no payload anywhere ─────────────────────────────────────────

#[test]
fn counter_is_opaque_with_no_payload() {
    let (c, crdt, inner) = parts(ref_of::<Counter>());
    assert_eq!(crdt, Some(CrdtCollectionType::Counter));
    assert_eq!(inner, None);
    let CollectionType::Record { fields } = c else {
        panic!("expected record")
    };
    assert!(fields.is_empty());
}

#[test]
fn counter_aliases_collapse_to_the_counter_tag() {
    // GCounter and PNCounter are type aliases of Counter; the compiler
    // resolves them, which is the failure mode this design removes.
    assert_eq!(ref_of::<GCounter>(), ref_of::<Counter>());
    assert_eq!(ref_of::<PNCounter>(), ref_of::<Counter>());
}

#[test]
fn rga_is_opaque_with_no_payload() {
    let (c, crdt, inner) = parts(ref_of::<ReplicatedGrowableArray>());
    assert_eq!(crdt, Some(CrdtCollectionType::ReplicatedGrowableArray));
    assert_eq!(inner, None);
    let CollectionType::Record { fields } = c else {
        panic!("expected record")
    };
    assert!(fields.is_empty());
}

#[test]
fn fugue_text_is_opaque_and_is_not_rga() {
    let (c, crdt, inner) = parts(ref_of::<FugueText>());
    assert_eq!(crdt, Some(CrdtCollectionType::FugueText));
    assert_eq!(inner, None);
    let CollectionType::Record { fields } = c else {
        panic!("expected record")
    };
    assert!(fields.is_empty());
    assert_ne!(
        ref_of::<FugueText>(),
        ref_of::<ReplicatedGrowableArray>(),
        "FugueText and RGA emit the same ABI, so a swap between them is invisible"
    );
}

// ── nesting falls out of recursion ──────────────────────────────────────

#[test]
fn nested_crdts_recurse_without_special_handling() {
    let (c, crdt, inner) = parts(ref_of::<UnorderedMap<String, AuthoredVector<String>>>());
    assert_eq!(crdt, Some(CrdtCollectionType::UnorderedMap));
    assert_eq!(inner, None);
    let CollectionType::Map { value, .. } = c else {
        panic!("expected map")
    };
    let (inner_c, inner_crdt, inner_inner) = parts(*value);
    assert_eq!(inner_crdt, Some(CrdtCollectionType::AuthoredVector));
    assert_eq!(inner_inner, None);
    assert!(matches!(inner_c, CollectionType::List { .. }));
}

// ── FrozenValue is transparent ──────────────────────────────────────────

#[test]
fn frozen_value_is_the_inner_type() {
    // Borsh-transparent wrapper: no collection, no crdt_type - the ABI ref
    // IS the inner type's ref, nested or scalar alike.
    assert_eq!(ref_of::<FrozenValue<String>>(), ref_of::<String>());
    assert_eq!(ref_of::<FrozenValue<Vec<u8>>>(), ref_of::<Vec<u8>>());
}

#[test]
fn writer_set_cell_shares_the_shared_storage_shape() {
    // PermissionedStorage is a thin wrapper over WriterSetCell; the storage
    // layout is identical, so the ABI shape must be too.
    assert_eq!(
        ref_of::<WriterSetCell<LwwRegister<String>>>(),
        ref_of::<SharedStorage<LwwRegister<String>>>()
    );
}
