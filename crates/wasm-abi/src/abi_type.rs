//! Types describe themselves for the ABI, so resolution is the compiler's job
//! rather than a source parser's. An alias does not exist here (it is already
//! the aliased type) and a macro has already expanded.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, LinkedList, VecDeque};

use indexmap::{IndexMap, IndexSet};

use crate::schema::{CollectionType, Event, ScalarType, TypeDef, TypeRef};

/// Collects named type definitions while a manifest is being built.
///
/// `define` is idempotent by name and guards recursion: a name is marked as
/// in-progress before its body is built, so a type that contains itself
/// terminates instead of recursing forever.
#[derive(Debug, Default)]
pub struct TypeRegistry {
    types: BTreeMap<String, TypeDef>,
    in_progress: BTreeSet<String>,
}

impl TypeRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `name`'s definition. Redefining a name with the same shape is
    /// a no-op; redefining it with a different shape panics, because a silent
    /// overwrite would emit a manifest describing the wrong type.
    ///
    /// # Panics
    /// If `name` is already defined with a different shape.
    pub fn define(&mut self, name: &str, build: impl FnOnce(&mut Self) -> TypeDef) {
        if self.in_progress.contains(name) {
            return;
        }
        let _ = self.in_progress.insert(name.to_owned());
        let def = build(self);
        let _ = self.in_progress.remove(name);
        match self.types.get(name) {
            Some(existing) => assert!(
                *existing == def,
                "ABI type name collision: {name} defined with two different shapes. \
                 Rename one side with #[abi(name = \"...\")] on its derive; if this is \
                 one generic used with two different type arguments, define a separate \
                 concrete type for each so every shape has its own name."
            ),
            None => {
                let _ = self.types.insert(name.to_owned(), def);
            }
        }
    }

    #[must_use]
    pub fn into_types(self) -> BTreeMap<String, TypeDef> {
        self.types
    }
}

/// How a type is described in the ABI.
///
/// `inner_type` placement rule for implementors: populate it ONLY when the
/// chosen `CollectionType` has nowhere to carry the payload. `List` uses
/// `items` and `Map` uses `key`/`value`; only a `Record { fields: vec![] }`
/// placeholder needs `inner_type`.
pub trait AbiType {
    /// How a use site refers to this type.
    fn type_ref(reg: &mut TypeRegistry) -> TypeRef;

    /// Register this type's named definition, if it has one.
    fn register(_reg: &mut TypeRegistry) {}

    /// How `Vec<Self>` describes. The container asks the element because only
    /// `u8` deviates from the list shape, and Rust has no specialization.
    fn vec_type_ref(reg: &mut TypeRegistry) -> TypeRef {
        TypeRef::list(Self::type_ref(reg))
    }

    /// How `[Self; size]` describes. Same dispatch as [`AbiType::vec_type_ref`];
    /// only `u8` carries the size instead of listing its elements.
    fn array_type_ref(reg: &mut TypeRegistry, _size: usize) -> TypeRef {
        TypeRef::list(Self::type_ref(reg))
    }
}

/// Implemented by `#[app::event]` codegen for an app's declared event enum, so
/// `__calimero_abi` can collect its `Event` entries through the state's
/// associated `Event` type without matching on the enum itself.
pub trait AbiEvents {
    fn abi_events(reg: &mut TypeRegistry) -> Vec<Event>;
}

macro_rules! scalar_impl {
    ($($ty:ty => $variant:expr),* $(,)?) => {
        $(impl AbiType for $ty {
            fn type_ref(_reg: &mut TypeRegistry) -> TypeRef {
                TypeRef::Scalar($variant)
            }
        })*
    };
}

// The narrow integers widen exactly the way the emitter normalizes a bare use
// of them: the ABI has no scalar narrower than 32 bits.
scalar_impl! {
    bool => ScalarType::Bool,
    i8 => ScalarType::I32,
    i16 => ScalarType::I32,
    u16 => ScalarType::U32,
    i32 => ScalarType::I32,
    i64 => ScalarType::I64,
    u32 => ScalarType::U32,
    u64 => ScalarType::U64,
    f32 => ScalarType::F32,
    f64 => ScalarType::F64,
    String => ScalarType::String,
    () => ScalarType::Unit,
    // Pointer-sized integers describe the wasm32 build even when the manifest
    // is assembled by a host binary, which is where the emitter pins them too.
    usize => ScalarType::U32,
    isize => ScalarType::I32,
}

impl AbiType for str {
    fn type_ref(_reg: &mut TypeRegistry) -> TypeRef {
        TypeRef::Scalar(ScalarType::String)
    }
}

/// A bare `u8` is an integer, but a run of them is bytes: `Vec<u8>` and
/// `[u8; N]` are the ABI's byte string, sized only when the array is.
impl AbiType for u8 {
    fn type_ref(_reg: &mut TypeRegistry) -> TypeRef {
        TypeRef::Scalar(ScalarType::U32)
    }

    fn vec_type_ref(_reg: &mut TypeRegistry) -> TypeRef {
        TypeRef::bytes()
    }

    fn array_type_ref(_reg: &mut TypeRegistry, size: usize) -> TypeRef {
        TypeRef::bytes_with_size(size, None)
    }
}

/// A reference describes the same type as its referent, so a generated event
/// carrying `&'a str` or similar borrows registers identically to the owned form.
impl<T: AbiType + ?Sized> AbiType for &T {
    fn type_ref(reg: &mut TypeRegistry) -> TypeRef {
        <T as AbiType>::type_ref(reg)
    }

    fn register(reg: &mut TypeRegistry) {
        <T as AbiType>::register(reg);
    }
}

impl<T: AbiType + ?Sized> AbiType for &mut T {
    fn type_ref(reg: &mut TypeRegistry) -> TypeRef {
        <T as AbiType>::type_ref(reg)
    }

    fn register(reg: &mut TypeRegistry) {
        <T as AbiType>::register(reg);
    }
}

impl<T: AbiType> AbiType for Vec<T> {
    fn type_ref(reg: &mut TypeRegistry) -> TypeRef {
        <T as AbiType>::vec_type_ref(reg)
    }

    fn register(reg: &mut TypeRegistry) {
        <T as AbiType>::register(reg);
    }
}

/// A slice is always a list, even `[u8]`: only `Vec<u8>` and `[u8; N]` are the
/// byte string.
impl<T: AbiType> AbiType for [T] {
    fn type_ref(reg: &mut TypeRegistry) -> TypeRef {
        TypeRef::list(<T as AbiType>::type_ref(reg))
    }

    fn register(reg: &mut TypeRegistry) {
        <T as AbiType>::register(reg);
    }
}

/// `Option<T>` describes as `T`; optionality is carried by `Field::nullable`
/// and `Method::returns_nullable`, not by the type reference.
impl<T: AbiType> AbiType for Option<T> {
    fn type_ref(reg: &mut TypeRegistry) -> TypeRef {
        <T as AbiType>::type_ref(reg)
    }

    fn register(reg: &mut TypeRegistry) {
        <T as AbiType>::register(reg);
    }
}

/// `Result<T, E>` describes as `T`: errors ride the return envelope, never the
/// type. `E` is unbounded because it is discarded, so aliased Results resolve.
impl<T: AbiType, E> AbiType for Result<T, E> {
    fn type_ref(reg: &mut TypeRegistry) -> TypeRef {
        <T as AbiType>::type_ref(reg)
    }

    fn register(reg: &mut TypeRegistry) {
        <T as AbiType>::register(reg);
    }
}

impl<T: AbiType, const N: usize> AbiType for [T; N] {
    fn type_ref(reg: &mut TypeRegistry) -> TypeRef {
        <T as AbiType>::array_type_ref(reg, N)
    }

    fn register(reg: &mut TypeRegistry) {
        <T as AbiType>::register(reg);
    }
}

/// Keys are `String` only: the ABI's map contract admits no other key type,
/// so a non-string key is a compile error here rather than an emit error.
macro_rules! map_impl {
    ($($ty:ident),* $(,)?) => {
        $(impl<V: AbiType> AbiType for $ty<String, V> {
            fn type_ref(reg: &mut TypeRegistry) -> TypeRef {
                TypeRef::Collection {
                    collection: CollectionType::Map {
                        key: Box::new(TypeRef::string()),
                        value: Box::new(<V as AbiType>::type_ref(reg)),
                    },
                    crdt_type: None,
                    inner_type: None,
                }
            }

            fn register(reg: &mut TypeRegistry) {
                <V as AbiType>::register(reg);
            }
        })*
    };
}

map_impl!(HashMap, BTreeMap, IndexMap);

/// Sets and non-Vec sequences describe as a list. The `u8` bytes shortcut is
/// deliberately not taken: it applies to `Vec` only, so `VecDeque<u8>` and
/// `BTreeSet<u8>` are lists of the widened scalar, not byte strings.
macro_rules! set_impl {
    ($($ty:ident),* $(,)?) => {
        $(impl<T: AbiType> AbiType for $ty<T> {
            fn type_ref(reg: &mut TypeRegistry) -> TypeRef {
                TypeRef::list(<T as AbiType>::type_ref(reg))
            }

            fn register(reg: &mut TypeRegistry) {
                <T as AbiType>::register(reg);
            }
        })*
    };
}

set_impl!(HashSet, BTreeSet, IndexSet, VecDeque, LinkedList);

macro_rules! tuple_impl {
    ($($name:ident),+) => {
        impl<$($name: AbiType),+> AbiType for ($($name,)+) {
            fn type_ref(reg: &mut TypeRegistry) -> TypeRef {
                TypeRef::Collection {
                    collection: CollectionType::Tuple {
                        elements: vec![$(<$name as AbiType>::type_ref(reg)),+],
                    },
                    crdt_type: None,
                    inner_type: None,
                }
            }

            fn register(reg: &mut TypeRegistry) {
                $(<$name as AbiType>::register(reg);)+
            }
        }
    };
}

tuple_impl!(A);
tuple_impl!(A, B);
tuple_impl!(A, B, C);
tuple_impl!(A, B, C, D);
tuple_impl!(A, B, C, D, E);
tuple_impl!(A, B, C, D, E, F);
tuple_impl!(A, B, C, D, E, F, G);
tuple_impl!(A, B, C, D, E, F, G, H);
tuple_impl!(A, B, C, D, E, F, G, H, I);
tuple_impl!(A, B, C, D, E, F, G, H, I, J);
tuple_impl!(A, B, C, D, E, F, G, H, I, J, K);
tuple_impl!(A, B, C, D, E, F, G, H, I, J, K, L);
