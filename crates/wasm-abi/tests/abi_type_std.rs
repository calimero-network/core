use std::collections::{BTreeMap, HashMap};

use calimero_wasm_abi::abi_type::{AbiType, TypeRegistry};
use calimero_wasm_abi::schema::{CollectionType, ScalarType, TypeRef};

fn ref_of<T: AbiType>() -> TypeRef {
    let mut reg = TypeRegistry::new();
    <T as AbiType>::type_ref(&mut reg)
}

#[test]
fn vec_is_a_list_of_its_item() {
    let TypeRef::Collection {
        collection,
        crdt_type,
        inner_type,
    } = ref_of::<Vec<String>>()
    else {
        panic!("expected a collection");
    };
    assert_eq!(crdt_type, None, "a plain Vec carries no CRDT metadata");
    assert_eq!(inner_type, None, "List.items carries the payload");
    let CollectionType::List { items } = collection else {
        panic!("expected a list")
    };
    assert_eq!(*items, TypeRef::Scalar(ScalarType::String));
}

#[test]
fn option_is_its_inner_type() {
    assert_eq!(ref_of::<Option<u64>>(), TypeRef::Scalar(ScalarType::U64));
}

#[test]
fn box_is_its_inner_type() {
    // Borsh-transparent: a boxed value serializes as the pointee.
    assert_eq!(ref_of::<Box<u64>>(), TypeRef::Scalar(ScalarType::U64));
    assert_eq!(ref_of::<Box<Vec<String>>>(), ref_of::<Vec<String>>());
}

#[test]
fn byte_array_is_sized_bytes() {
    let TypeRef::Scalar(ScalarType::Bytes { size, .. }) = ref_of::<[u8; 32]>() else {
        panic!("expected sized bytes");
    };
    assert_eq!(size, Some(32));
}

/// The container asks its element what shape to take, so `u8` alone turns a
/// sequence into bytes while every other element keeps the list shape.
#[test]
fn only_a_run_of_u8_is_bytes() {
    assert_eq!(
        ref_of::<Vec<u8>>(),
        TypeRef::Scalar(ScalarType::Bytes {
            size: None,
            encoding: None,
        }),
        "Vec<u8> is the unsized byte string",
    );
    assert_eq!(ref_of::<Vec<u16>>(), TypeRef::list(TypeRef::u32()));
    assert_eq!(
        ref_of::<[String; 3]>(),
        TypeRef::list(TypeRef::string()),
        "a non-u8 array lists its elements; the size is not carried",
    );
    // A slice is a list even for bytes, matching the emitter's `[T] -> list<T>`.
    assert_eq!(ref_of::<&[u8]>(), TypeRef::list(TypeRef::u32()));
}

/// Bare narrow integers widen: the ABI has no scalar under 32 bits.
#[test]
fn narrow_integers_widen() {
    assert_eq!(ref_of::<u8>(), TypeRef::u32());
    assert_eq!(ref_of::<u16>(), TypeRef::u32());
    assert_eq!(ref_of::<i8>(), TypeRef::i32());
    assert_eq!(ref_of::<i16>(), TypeRef::i32());
}

#[test]
fn both_map_types_produce_the_same_shape() {
    for r in [
        ref_of::<HashMap<String, u32>>(),
        ref_of::<BTreeMap<String, u32>>(),
    ] {
        let TypeRef::Collection {
            collection,
            crdt_type,
            inner_type,
        } = r
        else {
            panic!("expected a collection");
        };
        assert_eq!(crdt_type, None);
        assert_eq!(inner_type, None, "Map.key/value carry the payload");
        let CollectionType::Map { key, value } = collection else {
            panic!("expected a map")
        };
        assert_eq!(*key, TypeRef::Scalar(ScalarType::String));
        assert_eq!(*value, TypeRef::Scalar(ScalarType::U32));
    }
}

#[test]
fn tuple_is_positional() {
    let TypeRef::Collection { collection, .. } = ref_of::<(String, u64)>() else {
        panic!("expected a collection");
    };
    let CollectionType::Tuple { elements } = collection else {
        panic!("expected a tuple")
    };
    assert_eq!(elements.len(), 2);
    assert_eq!(elements[0], TypeRef::Scalar(ScalarType::String));
}

/// Tuples describe at any arity the emitter accepted, not just pairs and
/// triples; a 1-tuple stays a tuple rather than collapsing to its element.
#[test]
fn tuples_cover_the_emitter_arities() {
    let TypeRef::Collection { collection, .. } = ref_of::<(String,)>() else {
        panic!("expected a collection");
    };
    let CollectionType::Tuple { elements } = collection else {
        panic!("expected a tuple")
    };
    assert_eq!(elements, vec![TypeRef::string()]);

    let TypeRef::Collection { collection, .. } =
        ref_of::<(u8, bool, String, u64, f32, i64, u32, (String, u64))>()
    else {
        panic!("expected a collection");
    };
    let CollectionType::Tuple { elements } = collection else {
        panic!("expected a tuple")
    };
    assert_eq!(elements.len(), 8);
    assert_eq!(elements[0], TypeRef::u32(), "u8 widens inside a tuple too");
}

/// The non-Vec containers the emitter accepted keep their list/map shapes,
/// and none of them takes the Vec<u8> bytes shortcut.
#[test]
fn other_containers_keep_their_emitter_shapes() {
    use std::collections::{LinkedList, VecDeque};

    use indexmap::{IndexMap, IndexSet};

    assert_eq!(ref_of::<VecDeque<u8>>(), TypeRef::list(TypeRef::u32()));
    assert_eq!(
        ref_of::<LinkedList<String>>(),
        TypeRef::list(TypeRef::string())
    );
    assert_eq!(
        ref_of::<IndexSet<String>>(),
        TypeRef::list(TypeRef::string())
    );
    assert_eq!(
        ref_of::<IndexMap<String, u32>>(),
        ref_of::<HashMap<String, u32>>()
    );
}

/// A Result unwraps to its ok type even through an alias the macro's syntactic
/// unwrap cannot see; the error type needs no impl because it is discarded.
#[test]
fn result_describes_as_its_ok_type() {
    struct NotAbi;
    type Aliased = Result<String, NotAbi>;
    assert_eq!(ref_of::<Result<u64, String>>(), TypeRef::u64());
    assert_eq!(ref_of::<Aliased>(), TypeRef::string());
}
