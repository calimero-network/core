//! Locks the exact ABI shape of every CRDT wrapper against what the syn
//! normalizer produces today. `inner_type` is populated ONLY where
//! `CollectionType` cannot carry the payload - two wrappers out of eleven -
//! and CRDT maps always describe their key as `string` no matter the Rust
//! key type. A uniform-looking refactor of these impls is a bug, and this
//! file is what catches it.

use calimero_storage::collections::{
    AccessControl, AuthoredMap, AuthoredVector, Counter, FrozenStorage, GCounter, LwwRegister,
    Ownable, PNCounter, ReplicatedGrowableArray, SharedStorage, SortedMap, SortedSet, UnorderedMap,
    UnorderedSet, UserStorage, Vector,
};
use calimero_wasm_abi::abi_type::{AbiType, TypeRegistry};
use calimero_wasm_abi::schema::{CollectionType, CrdtCollectionType, ScalarType, TypeRef};

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
