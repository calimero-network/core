use syn::{GenericArgument, Type, TypePath};

use crate::schema::{CollectionType, CrdtCollectionType, ScalarType, TypeRef};

/// Error types for type normalization
#[derive(Debug, thiserror::Error)]
pub enum NormalizeError {
    #[error("type path error: {0}")]
    TypePathError(String),
    #[error("unsupported map key type: {0}")]
    UnsupportedMapKey(String),
    #[error("unsupported array element type: {0}")]
    UnsupportedArrayElement(String),
}

/// Resolved local type information
#[derive(Debug, Clone, Copy)]
pub enum ResolvedLocal {
    /// Newtype bytes wrapper (e.g., struct UserId([u8;32]))
    NewtypeBytes { size: usize },
    /// Record struct (shape filled elsewhere)
    Record,
    /// Variant enum (shape filled elsewhere)
    Variant,
}

/// Trait for resolving local type names
pub trait TypeResolver {
    fn resolve_local(&self, name: &str) -> Option<ResolvedLocal>;
}

/// Normalize a Rust type to an ABI TypeRef
pub fn normalize_type(
    ty: &Type,
    wasm32: bool,
    resolver: &dyn TypeResolver,
) -> Result<TypeRef, NormalizeError> {
    match ty {
        Type::Path(type_path) => normalize_path_type(type_path, wasm32, resolver),
        Type::Reference(type_ref) => {
            // Strip references and lifetimes
            normalize_type(&type_ref.elem, wasm32, resolver)
        }
        Type::Slice(type_slice) => {
            // [T] -> list<T>
            let item_type = normalize_type(&type_slice.elem, wasm32, resolver)?;
            Ok(TypeRef::list(item_type))
        }
        Type::Array(type_array) => {
            // [T; N] -> list<T> or bytes{size:N} for [u8; N]
            let elem_type = &*type_array.elem;
            let len = &type_array.len;

            // Check if it's [u8; N]
            if let Type::Path(TypePath { path, .. }) = elem_type {
                eprintln!("Checking if array element is u8");
                if is_u8_type(path) {
                    eprintln!("Array element is u8, extracting length");
                    let size = extract_array_len(len)?;
                    eprintln!("Array size: {size}");
                    return Ok(TypeRef::bytes_with_size(size, None));
                }
            }

            // Otherwise, treat as list
            let item_type = normalize_type(elem_type, wasm32, resolver)?;
            Ok(TypeRef::list(item_type))
        }
        Type::Tuple(type_tuple) => {
            // () -> unit
            if type_tuple.elems.is_empty() {
                Ok(TypeRef::unit())
            } else {
                Err(NormalizeError::TypePathError(
                    "unsupported tuple".to_owned(),
                ))
            }
        }
        _ => Err(NormalizeError::TypePathError("unsupported type".to_owned())),
    }
}

/// Normalize a path type (e.g., Option<T>, Vec<T>, etc.)
fn normalize_path_type(
    type_path: &TypePath,
    wasm32: bool,
    resolver: &dyn TypeResolver,
) -> Result<TypeRef, NormalizeError> {
    let path = &type_path.path;

    eprintln!("Path segments: {}", path.segments.len());
    for (i, seg) in path.segments.iter().enumerate() {
        eprintln!("  Segment {}: {}", i, seg.ident);
    }

    if path.segments.len() == 1 {
        let segment = &path.segments[0];
        let ident = &segment.ident;

        eprintln!(
            "Processing path type: {} with {} segments",
            ident,
            path.segments.len()
        );

        // Handle generic types
        if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
            eprintln!("Found angle-bracketed arguments");
            return normalize_generic_type(ident, args, wasm32, resolver);
        }

        // Handle scalar types
        eprintln!("No angle-bracketed arguments, treating as scalar");
        return normalize_scalar_type(path, wasm32, resolver);
    } else if path.segments.len() == 2 {
        // Handle qualified paths like app::Result
        let first_segment = &path.segments[0];
        let second_segment = &path.segments[1];

        eprintln!(
            "Processing qualified path: {}::{}",
            first_segment.ident, second_segment.ident
        );

        // Handle app::Result -> Result
        if first_segment.ident == "app" && second_segment.ident == "Result" {
            if let syn::PathArguments::AngleBracketed(args) = &second_segment.arguments {
                return normalize_generic_type(&second_segment.ident, args, wasm32, resolver);
            }
        }
    }

    Err(NormalizeError::TypePathError(
        "invalid type path".to_owned(),
    ))
}

/// Normalize generic types like Option<T>, Vec<T>, etc.
fn normalize_generic_type(
    ident: &syn::Ident,
    args: &syn::AngleBracketedGenericArguments,
    wasm32: bool,
    resolver: &dyn TypeResolver,
) -> Result<TypeRef, NormalizeError> {
    let ident_str = ident.to_string();
    eprintln!(
        "Processing generic type: '{}' (len: {}) with {} args",
        ident_str,
        ident_str.len(),
        args.args.len()
    );
    match ident_str.as_str() {
        "Option" => {
            // Option<T> -> T (nullable handled at field level)
            if args.args.len() != 1 {
                return Err(NormalizeError::TypePathError(
                    "invalid Option type".to_owned(),
                ));
            }
            let arg = &args.args[0];
            let GenericArgument::Type(ty) = arg else {
                return Err(NormalizeError::TypePathError(
                    "invalid Option argument".to_owned(),
                ));
            };
            normalize_type(ty, wasm32, resolver)
        }
        // List/Vector types - normalize to semantic list type
        "Vec" | "VecDeque" | "LinkedList" => {
            // All list types -> list<T> or bytes for Vec<u8>
            if args.args.len() != 1 {
                return Err(NormalizeError::TypePathError(format!(
                    "invalid {ident_str} type - expected 1 type argument"
                )));
            }
            let item_arg = &args.args[0];
            let GenericArgument::Type(item_ty) = item_arg else {
                return Err(NormalizeError::TypePathError(format!(
                    "invalid {ident_str} item type"
                )));
            };

            // Special case: Vec<u8> -> bytes (only for Vec, not other list types)
            if ident_str == "Vec" {
                if let Type::Path(TypePath { path, .. }) = item_ty {
                    if is_u8_type(path) {
                        return Ok(TypeRef::Scalar(ScalarType::Bytes {
                            size: None,
                            encoding: None,
                        }));
                    }
                }
            }

            let item_type = normalize_type(item_ty, wasm32, resolver)?;
            Ok(TypeRef::list(item_type))
        }
        // Collection types - normalize to semantic ABI types
        "BTreeMap" | "HashMap" | "UnorderedMap" | "IndexMap" => {
            // All map types -> map<K, V> (normalize to semantic type)
            // UnorderedMap preserves CRDT type metadata
            if args.args.len() != 2 {
                return Err(NormalizeError::TypePathError(format!(
                    "invalid {ident_str} type - expected 2 type arguments"
                )));
            }

            let key_arg = &args.args[0];
            let value_arg = &args.args[1];

            let GenericArgument::Type(key_ty) = key_arg else {
                return Err(NormalizeError::TypePathError(format!(
                    "invalid {ident_str} key type"
                )));
            };

            let GenericArgument::Type(value_ty) = value_arg else {
                return Err(NormalizeError::TypePathError(format!(
                    "invalid {ident_str} value type"
                )));
            };

            // Normalize key and value types
            let _key_type = normalize_type(key_ty, wasm32, resolver)?;
            let value_type = normalize_type(value_ty, wasm32, resolver)?;

            // Preserve CRDT type for UnorderedMap
            let crdt_type = if ident_str == "UnorderedMap" {
                Some(CrdtCollectionType::UnorderedMap)
            } else {
                None
            };

            Ok(TypeRef::Collection {
                collection: CollectionType::Map {
                    key: Box::new(TypeRef::Scalar(ScalarType::String)),
                    value: Box::new(value_type),
                },
                crdt_type,
                inner_type: None, // Inner types are in Map.key and Map.value
            })
        }
        // Set types - normalize to semantic list type (sets are just lists without duplicates)
        "HashSet" | "BTreeSet" | "IndexSet" => {
            // All set types -> list<T> (normalize to semantic type)
            if args.args.len() != 1 {
                return Err(NormalizeError::TypePathError(format!(
                    "invalid {ident_str} type - expected 1 type argument"
                )));
            }

            let arg = &args.args[0];
            let GenericArgument::Type(item_ty) = arg else {
                return Err(NormalizeError::TypePathError(format!(
                    "invalid {ident_str} argument"
                )));
            };

            let item_type = normalize_type(item_ty, wasm32, resolver)?;
            Ok(TypeRef::list(item_type))
        }
        "Result" => {
            // Result<T, E> -> T (error handling separate)
            // Handle both Result<T, E> and Result<T> (where E has a default)
            if args.args.len() == 1 {
                // Result<T> - single argument, error type has default
                let arg = &args.args[0];
                let GenericArgument::Type(ty) = arg else {
                    return Err(NormalizeError::TypePathError(
                        "invalid Result argument".to_owned(),
                    ));
                };
                normalize_type(ty, wasm32, resolver)
            } else if args.args.len() == 2 {
                // Result<T, E> - two arguments
                let arg = &args.args[0];
                let GenericArgument::Type(ty) = arg else {
                    return Err(NormalizeError::TypePathError(
                        "invalid Result argument".to_owned(),
                    ));
                };
                normalize_type(ty, wasm32, resolver)
            } else {
                return Err(NormalizeError::TypePathError(
                    "invalid Result type".to_owned(),
                ));
            }
        }
        // CRDT types - unwrap to inner type for ABI but preserve CRDT type metadata
        "LwwRegister"
        | "Counter"
        | "ReplicatedGrowableArray"
        | "Vector"
        | "UnorderedSet"
        | "FrozenValue" => {
            // These CRDT wrappers unwrap to their inner type for ABI purposes
            // but we preserve the CRDT type so deserializers know the format

            if ident_str == "Counter" || ident_str == "ReplicatedGrowableArray" {
                // Counter and RGA don't have generic args (or are opaque)
                // Counter -> bytes (but preserve Counter type), RGA -> string (but preserve RGA type)
                if ident_str == "Counter" {
                    // Counter serializes as (positive: UnorderedMap<String, u64>, negative?: UnorderedMap<String, u64>)
                    // We represent it as bytes with CRDT type metadata
                    return Ok(TypeRef::Collection {
                        collection: CollectionType::Record {
                            fields: vec![], // Placeholder - Counter has complex internal structure
                        },
                        crdt_type: Some(CrdtCollectionType::Counter),
                        inner_type: None, // Counter doesn't wrap another type
                    });
                } else {
                    // RGA serializes as a string with CRDT metadata
                    return Ok(TypeRef::Collection {
                        collection: CollectionType::Record {
                            fields: vec![], // Placeholder
                        },
                        crdt_type: Some(CrdtCollectionType::ReplicatedGrowableArray),
                        inner_type: None,
                    });
                }
            }

            // LwwRegister<T>, Vector<T>, UnorderedSet<T> -> unwrap T but preserve CRDT type
            if args.args.is_empty() {
                return Err(NormalizeError::TypePathError(format!(
                    "invalid {ident_str} type - expected 1 type argument"
                )));
            }
            let arg = &args.args[0];
            let GenericArgument::Type(ty) = arg else {
                return Err(NormalizeError::TypePathError(
                    "invalid CRDT argument".to_owned(),
                ));
            };
            let inner_type = normalize_type(ty, wasm32, resolver)?;

            // Wrap the inner type in a Collection with CRDT metadata
            match ident_str.as_str() {
                "LwwRegister" => {
                    // LwwRegister<T> wraps a single value T with CRDT metadata
                    // We preserve the inner type so deserializer knows how to deserialize the value
                    // The deserializer will handle the (value: T, timestamp, node_id) format
                    Ok(TypeRef::Collection {
                        collection: CollectionType::Record {
                            fields: vec![], // Placeholder - inner_type stores the actual type
                        },
                        crdt_type: Some(CrdtCollectionType::LwwRegister),
                        inner_type: Some(Box::new(inner_type)),
                    })
                }
                "Vector" => {
                    // Vector<T> -> List<T> with CRDT type
                    // The inner_type is already in the List's items field
                    Ok(TypeRef::Collection {
                        collection: CollectionType::List {
                            items: Box::new(inner_type),
                        },
                        crdt_type: Some(CrdtCollectionType::Vector),
                        inner_type: None, // Inner type is in List.items
                    })
                }
                "UnorderedSet" => {
                    // UnorderedSet<T> -> List<T> with CRDT type
                    // The inner_type is already in the List's items field
                    Ok(TypeRef::Collection {
                        collection: CollectionType::List {
                            items: Box::new(inner_type),
                        },
                        crdt_type: Some(CrdtCollectionType::UnorderedSet),
                        inner_type: None, // Inner type is in List.items
                    })
                }
                "FrozenValue" => {
                    // FrozenValue is not a CRDT, just a wrapper - no CRDT metadata
                    Ok(inner_type)
                }
                _ => Ok(inner_type),
            }
        }
        // Handle UserStorage and FrozenStorage
        "UserStorage" | "FrozenStorage" => {
            // These normalize to map<string, T>
            // Key (PublicKey or Hash) is treated as a string
            if args.args.is_empty() {
                return Err(NormalizeError::TypePathError(format!(
                    "invalid {ident_str} type - expected 1 type argument"
                )));
            }
            let arg = &args.args[0];
            let GenericArgument::Type(ty) = arg else {
                return Err(NormalizeError::TypePathError(
                    "invalid storage argument".to_owned(),
                ));
            };
            let value_type = normalize_type(ty, wasm32, resolver)?;

            // UserStorage and FrozenStorage are not CRDTs, just storage wrappers
            Ok(TypeRef::Collection {
                collection: CollectionType::Map {
                    key: Box::new(TypeRef::Scalar(ScalarType::String)),
                    value: Box::new(value_type),
                },
                crdt_type: None,
                inner_type: None,
            })
        }
        _ => Err(NormalizeError::TypePathError(format!(
            "unknown generic type: {ident}"
        ))),
    }
}

/// Check if a path represents a u8 type
fn is_u8_type(path: &syn::Path) -> bool {
    path.segments.len() == 1 && path.segments[0].ident == "u8"
}

/// Extract array length from [T; N]
fn extract_array_len(len: &syn::Expr) -> Result<usize, NormalizeError> {
    if let syn::Expr::Lit(syn::ExprLit {
        lit: syn::Lit::Int(lit),
        ..
    }) = len
    {
        lit.base10_parse()
            .map_err(|_| NormalizeError::TypePathError("failed to parse array length".to_owned()))
    } else {
        Err(NormalizeError::TypePathError(
            "array length must be a literal integer".to_owned(),
        ))
    }
}

/// Normalize a scalar type
fn normalize_scalar_type(
    path: &syn::Path,
    wasm32: bool,
    resolver: &dyn TypeResolver,
) -> Result<TypeRef, NormalizeError> {
    if path.segments.len() != 1 {
        return Err(NormalizeError::TypePathError(
            "invalid scalar type path".to_owned(),
        ));
    }

    let ident = &path.segments[0].ident;
    match ident.to_string().as_str() {
        "bool" => Ok(TypeRef::bool()),
        "i8" | "i16" | "i32" => Ok(TypeRef::i32()),
        "i64" => Ok(TypeRef::i64()),
        "u8" | "u16" | "u32" => Ok(TypeRef::u32()),
        "u64" => Ok(TypeRef::u64()),
        "f32" => Ok(TypeRef::f32()),
        "f64" => Ok(TypeRef::f64()),
        "String" | "str" => Ok(TypeRef::string()),
        "usize" => {
            if wasm32 {
                Ok(TypeRef::u32())
            } else {
                Ok(TypeRef::u64())
            }
        }
        "isize" => {
            if wasm32 {
                Ok(TypeRef::i32())
            } else {
                Ok(TypeRef::i64())
            }
        }
        // Handle PublicKey
        "PublicKey" => {
            // PublicKey is [u8; 32], so it's bytes with a fixed size
            Ok(TypeRef::bytes_with_size(32, None))
        }
        // Storage CRDT wrappers – treat as opaque blobs until ABI definitions exist.
        "Counter" => Ok(TypeRef::Collection {
            collection: CollectionType::Record {
                fields: vec![], // Placeholder - Counter has complex internal structure
            },
            crdt_type: Some(CrdtCollectionType::Counter),
            inner_type: None, // Counter doesn't wrap another type
        }),
        "ReplicatedGrowableArray" => Ok(TypeRef::Collection {
            collection: CollectionType::Record {
                fields: vec![], // Placeholder
            },
            crdt_type: Some(CrdtCollectionType::ReplicatedGrowableArray),
            inner_type: None,
        }),
        _ => {
            // Check if it's a local type
            resolver.resolve_local(&ident.to_string()).map_or_else(
                || {
                    Err(NormalizeError::TypePathError(format!(
                        "unknown type: {ident}"
                    )))
                },
                |resolved| match resolved {
                    ResolvedLocal::NewtypeBytes { size } => {
                        Ok(TypeRef::bytes_with_size(size, None))
                    }
                    ResolvedLocal::Record | ResolvedLocal::Variant => {
                        Ok(TypeRef::reference(&ident.to_string()))
                    }
                },
            )
        }
    }
}

/// Extension trait for TypeRef to set nullable flag
pub trait TypeRefExt {
    fn set_nullable(&mut self, nullable: bool);
}

impl TypeRefExt for TypeRef {
    fn set_nullable(&mut self, _nullable: bool) {
        // Nullable is now handled at the Parameter/Field/Method.returns_nullable level
    }
}
