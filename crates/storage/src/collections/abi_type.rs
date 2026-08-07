//! `AbiType` for the CRDT wrappers. These carry semantics a derive cannot
//! infer, so they are written by hand.
//!
//! `inner_type` is a FALLBACK, not the default: it is populated only where the
//! chosen `CollectionType` has nowhere to put the payload. `List` has `items`
//! and `Map` has `key`/`value`; only the empty-`Record` placeholder needs it.
//! Making these uniform would produce a valid but different ABI - see
//! `tests/abi_crdt_shapes.rs`.

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_wasm_abi::abi_type::{AbiType, TypeRegistry};
use calimero_wasm_abi::schema::{CollectionType, CrdtCollectionType, ScalarType, TypeRef};

use super::crdt_meta::Mergeable;
use super::permissioned::{Authorizer, PermissionedStorage};
use super::{
    AccessControl, AuthoredMap, AuthoredVector, Counter, FrozenStorage, FrozenValue, LwwRegister,
    ReplicatedGrowableArray, SortedMap, SortedSet, UnorderedMap, UnorderedSet, UserStorage, Vector,
    WriterSetCell,
};
use crate::store::StorageAdaptor;

/// Payload in `List.items`.
fn list_ref<T: AbiType>(reg: &mut TypeRegistry, crdt: CrdtCollectionType) -> TypeRef {
    TypeRef::Collection {
        collection: CollectionType::List {
            items: Box::new(<T as AbiType>::type_ref(reg)),
        },
        crdt_type: Some(crdt),
        inner_type: None,
    }
}

/// Payload in `Map.value`. The key is always `string`: the CRDT layer keys
/// entries internally, so the Rust key type never reaches the ABI.
fn map_ref<V: AbiType>(reg: &mut TypeRegistry, crdt: Option<CrdtCollectionType>) -> TypeRef {
    TypeRef::Collection {
        collection: CollectionType::Map {
            key: Box::new(TypeRef::string()),
            value: Box::new(<V as AbiType>::type_ref(reg)),
        },
        crdt_type: crdt,
        inner_type: None,
    }
}

/// The exception: a `Record` needs real fields and these wrappers have none,
/// so the payload rides `inner_type` beside an empty placeholder.
fn cell_ref<T: AbiType>(reg: &mut TypeRegistry, crdt: CrdtCollectionType) -> TypeRef {
    TypeRef::Collection {
        collection: CollectionType::Record { fields: vec![] },
        crdt_type: Some(crdt),
        inner_type: Some(Box::new(<T as AbiType>::type_ref(reg))),
    }
}

/// No payload anywhere.
fn opaque_ref(crdt: CrdtCollectionType) -> TypeRef {
    TypeRef::Collection {
        collection: CollectionType::Record { fields: vec![] },
        crdt_type: Some(crdt),
        inner_type: None,
    }
}

impl<V: AbiType, S: StorageAdaptor> AbiType for Vector<V, S> {
    fn type_ref(reg: &mut TypeRegistry) -> TypeRef {
        list_ref::<V>(reg, CrdtCollectionType::Vector)
    }

    fn register(reg: &mut TypeRegistry) {
        <V as AbiType>::register(reg);
    }
}

impl<V: AbiType, S: StorageAdaptor> AbiType for UnorderedSet<V, S> {
    fn type_ref(reg: &mut TypeRegistry) -> TypeRef {
        list_ref::<V>(reg, CrdtCollectionType::UnorderedSet)
    }

    fn register(reg: &mut TypeRegistry) {
        <V as AbiType>::register(reg);
    }
}

impl<V: AbiType, S: StorageAdaptor> AbiType for SortedSet<V, S> {
    fn type_ref(reg: &mut TypeRegistry) -> TypeRef {
        list_ref::<V>(reg, CrdtCollectionType::SortedSet)
    }

    fn register(reg: &mut TypeRegistry) {
        <V as AbiType>::register(reg);
    }
}

impl<V, S> AbiType for AuthoredVector<V, S>
where
    V: BorshSerialize + BorshDeserialize + AbiType,
    S: StorageAdaptor,
{
    fn type_ref(reg: &mut TypeRegistry) -> TypeRef {
        list_ref::<V>(reg, CrdtCollectionType::AuthoredVector)
    }

    fn register(reg: &mut TypeRegistry) {
        <V as AbiType>::register(reg);
    }
}

impl<K, V: AbiType, S: StorageAdaptor> AbiType for UnorderedMap<K, V, S> {
    fn type_ref(reg: &mut TypeRegistry) -> TypeRef {
        map_ref::<V>(reg, Some(CrdtCollectionType::UnorderedMap))
    }

    fn register(reg: &mut TypeRegistry) {
        <V as AbiType>::register(reg);
    }
}

impl<K, V: AbiType, S: StorageAdaptor> AbiType for SortedMap<K, V, S> {
    fn type_ref(reg: &mut TypeRegistry) -> TypeRef {
        map_ref::<V>(reg, Some(CrdtCollectionType::SortedMap))
    }

    fn register(reg: &mut TypeRegistry) {
        <V as AbiType>::register(reg);
    }
}

impl<K, V, S> AbiType for AuthoredMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize + AbiType,
    S: StorageAdaptor,
{
    fn type_ref(reg: &mut TypeRegistry) -> TypeRef {
        map_ref::<V>(reg, Some(CrdtCollectionType::AuthoredMap))
    }

    fn register(reg: &mut TypeRegistry) {
        <V as AbiType>::register(reg);
    }
}

impl<T: AbiType> AbiType for LwwRegister<T> {
    fn type_ref(reg: &mut TypeRegistry) -> TypeRef {
        cell_ref::<T>(reg, CrdtCollectionType::LwwRegister)
    }

    fn register(reg: &mut TypeRegistry) {
        <T as AbiType>::register(reg);
    }
}

/// Borsh-transparent immutable wrapper: the stored bytes are exactly the inner
/// value's, so the ABI sees the inner type itself, with no collection wrapper.
impl<T: AbiType> AbiType for FrozenValue<T> {
    fn type_ref(reg: &mut TypeRegistry) -> TypeRef {
        <T as AbiType>::type_ref(reg)
    }

    fn register(reg: &mut TypeRegistry) {
        <T as AbiType>::register(reg);
    }
}

/// The engine under `PermissionedStorage`, and storable directly: identical
/// layout, so it carries the same `SharedStorage` shape.
impl<T, S> AbiType for WriterSetCell<T, S>
where
    T: BorshSerialize + BorshDeserialize + Mergeable + AbiType,
    S: StorageAdaptor,
{
    fn type_ref(reg: &mut TypeRegistry) -> TypeRef {
        cell_ref::<T>(reg, CrdtCollectionType::SharedStorage)
    }

    fn register(reg: &mut TypeRegistry) {
        <T as AbiType>::register(reg);
    }
}

/// Covers `SharedStorage<T>` and `Ownable<T>`: both alias `PermissionedStorage`,
/// and the policy is a zero-sized marker, so every policy shares one ABI shape.
impl<T, A> AbiType for PermissionedStorage<T, A>
where
    T: BorshSerialize + BorshDeserialize + Mergeable + Default + AbiType,
    A: Authorizer,
{
    fn type_ref(reg: &mut TypeRegistry) -> TypeRef {
        cell_ref::<T>(reg, CrdtCollectionType::SharedStorage)
    }

    fn register(reg: &mut TypeRegistry) {
        <T as AbiType>::register(reg);
    }
}

/// Covers `GCounter` and `PNCounter`: type aliases the compiler resolves.
impl<const ALLOW_DECREMENT: bool, S: StorageAdaptor> AbiType for Counter<ALLOW_DECREMENT, S> {
    fn type_ref(_reg: &mut TypeRegistry) -> TypeRef {
        opaque_ref(CrdtCollectionType::Counter)
    }
}

impl<S: StorageAdaptor> AbiType for ReplicatedGrowableArray<S> {
    fn type_ref(_reg: &mut TypeRegistry) -> TypeRef {
        opaque_ref(CrdtCollectionType::ReplicatedGrowableArray)
    }
}

/// `UserStorage` and `FrozenStorage` partition one value per identity rather
/// than converging writes, so they carry no `crdt_type`: the ABI sees a plain
/// string-keyed map, the identity key rendered as a string.
macro_rules! keyed_storage_impl {
    ($($ty:ident),* $(,)?) => {
        $(impl<T, S> AbiType for $ty<T, S>
        where
            T: BorshSerialize + BorshDeserialize + AbiType,
            S: StorageAdaptor,
        {
            fn type_ref(reg: &mut TypeRegistry) -> TypeRef {
                map_ref::<T>(reg, None)
            }

            fn register(reg: &mut TypeRegistry) {
                <T as AbiType>::register(reg);
            }
        })*
    };
}

keyed_storage_impl!(UserStorage, FrozenStorage);

/// A role registry: a writer-set-guarded `map<string, bool>` of grants, so it
/// carries the `SharedStorage` tag that makes the ACL visible.
impl AbiType for AccessControl {
    fn type_ref(_reg: &mut TypeRegistry) -> TypeRef {
        TypeRef::Collection {
            collection: CollectionType::Map {
                key: Box::new(TypeRef::string()),
                value: Box::new(TypeRef::Scalar(ScalarType::Bool)),
            },
            crdt_type: Some(CrdtCollectionType::SharedStorage),
            inner_type: None,
        }
    }
}
