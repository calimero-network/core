use crate::schema::TypeRef;
use syn::{GenericArgument, Type, TypePath};

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
                if is_u8_type(path) {
                    let size = extract_array_len(len)?;
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
                Err(NormalizeError::TypePathError("unsupported tuple".to_owned()))
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

    if path.segments.len() == 1 {
        let segment = &path.segments[0];
        let ident = &segment.ident;

        // Handle generic types
        if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
            return normalize_generic_type(ident, args, wasm32, resolver);
        }

        // Handle scalar types
        return normalize_scalar_type(path, wasm32, resolver);
    }

    Err(NormalizeError::TypePathError("invalid type path".to_owned()))
}

/// Normalize generic types like Option<T>, Vec<T>, etc.
fn normalize_generic_type(
    ident: &syn::Ident,
    args: &syn::AngleBracketedGenericArguments,
    wasm32: bool,
    resolver: &dyn TypeResolver,
) -> Result<TypeRef, NormalizeError> {
    if args.args.len() != 1 {
        return Err(NormalizeError::TypePathError(
            "invalid generic type".to_owned(),
        ));
    }

    let arg = &args.args[0];
    let GenericArgument::Type(ty) = arg else {
        return Err(NormalizeError::TypePathError(
            "invalid generic argument".to_owned(),
        ));
    };

    match ident.to_string().as_str() {
        "Option" => {
            // Option<T> -> T (nullable handled at field level)
            normalize_type(ty, wasm32, resolver)
        }
        "Vec" => {
            // Vec<T> -> list<T>
            let item_type = normalize_type(ty, wasm32, resolver)?;
            Ok(TypeRef::list(item_type))
        }
        "BTreeMap" => {
            // BTreeMap<K, V> -> map<string, V> (K must be String)
            if args.args.len() != 2 {
                return Err(NormalizeError::TypePathError(
                    "invalid BTreeMap type".to_owned(),
                ));
            }

            let key_arg = &args.args[0];
            let value_arg = &args.args[1];

            let GenericArgument::Type(key_ty) = key_arg else {
                return Err(NormalizeError::TypePathError(
                    "invalid BTreeMap key".to_owned(),
                ));
            };

            let GenericArgument::Type(value_ty) = value_arg else {
                return Err(NormalizeError::TypePathError(
                    "invalid BTreeMap value".to_owned(),
                ));
            };

            // Check that key is String
            if let Type::Path(TypePath { path, .. }) = key_ty {
                if !is_string_type(path) {
                    return Err(NormalizeError::UnsupportedMapKey(
                        "non-string key type".to_owned(),
                    ));
                }
            } else {
                return Err(NormalizeError::UnsupportedMapKey(
                    "non-string key type".to_owned(),
                ));
            }

            let value_type = normalize_type(value_ty, wasm32, resolver)?;
            Ok(TypeRef::map(value_type))
        }
        "Result" => {
            // Result<T, E> -> T (error handling separate)
            normalize_type(ty, wasm32, resolver)
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

/// Check if a path represents a string type
fn is_string_type(path: &syn::Path) -> bool {
    path.segments.len() == 1 && path.segments[0].ident == "String"
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
