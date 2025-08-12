use syn::{Type, TypeArray, TypePath, TypeReference, TypeTuple};
use thiserror::Error;

use crate::schema::TypeRef;

/// Error type for normalization failures
#[derive(Debug, Error)]
pub enum NormalizeError {
    #[error("unsupported map key type: {0} (only map<string, V> supported in v1)")]
    UnsupportedMapKey(String),
    #[error("unsupported array element type: {0} (only [u8; N] supported in v1)")]
    UnsupportedArrayElement(String),
    #[error("failed to parse type path: {0}")]
    TypePathError(String),
}

/// Resolved local type information
#[derive(Debug, Clone)]
pub enum ResolvedLocal {
    /// Newtype bytes wrapper (e.g., struct UserId([u8;32]))
    NewtypeBytes { size: usize },
    /// Record struct (shape filled elsewhere)
    Record,
    /// Variant enum (shape filled elsewhere)
    Variant,
}

/// Trait for resolving local type definitions
pub trait TypeResolver {
    /// Resolve a local type path to its definition
    fn resolve_local(&self, path: &str) -> Option<ResolvedLocal>;
}

/// Normalize a Rust type to an ABI TypeRef
pub fn normalize_type(
    ty: &syn::Type,
    wasm32: bool,
    resolver: &dyn TypeResolver,
) -> Result<TypeRef, NormalizeError> {
    match ty {
        // Handle references and lifetimes
        Type::Reference(TypeReference { elem, .. }) => normalize_type(elem, wasm32, resolver),

        // Handle Option<T>
        Type::Path(TypePath { path, .. }) if is_option(path) => {
            let inner_type = extract_option_inner(ty)?;
            let mut inner_ref = normalize_type(inner_type, wasm32, resolver)?;
            inner_ref.set_nullable(true);
            Ok(inner_ref)
        }

        // Handle Vec<u8> (bytes without size) - must come before generic Vec<T>
        Type::Path(TypePath { path, .. }) if is_vec_u8(path) => Ok(TypeRef::bytes()),

        // Handle Vec<T>
        Type::Path(TypePath { path, .. }) if is_vec(path) => {
            let inner_type = extract_vec_inner(ty)?;
            let inner_ref = normalize_type(inner_type, wasm32, resolver)?;
            Ok(TypeRef::list(inner_ref))
        }

        // Handle BTreeMap<String, V>
        Type::Path(TypePath { path, .. }) if is_btree_map(path) => {
            let (key_type, value_type) = extract_map_inner(ty)?;

            // Check that key type is String (after normalization)
            let normalized_key = normalize_type(key_type, wasm32, resolver)?;
            if !is_string_type(&normalized_key) {
                return Err(NormalizeError::UnsupportedMapKey(
                    "non-string key type".to_string(),
                ));
            }

            let normalized_value = normalize_type(value_type, wasm32, resolver)?;
            Ok(TypeRef::map(normalized_value))
        }

        // Handle arrays [T; N]
        Type::Array(TypeArray { elem, len, .. }) => {
            let elem_type = &**elem;

            // Check if it's [u8; N]
            if let Type::Path(TypePath { path, .. }) = elem_type {
                if is_u8_type(path) {
                    let size = extract_array_len(len)?;
                    return Ok(TypeRef::bytes_with_size(size, "hex"));
                }
            }

            Err(NormalizeError::UnsupportedArrayElement(
                "non-u8 element type".to_string(),
            ))
        }

        // Handle scalar types
        Type::Path(TypePath { path, .. }) => normalize_scalar_type(path, wasm32, resolver),

        // Handle unit type ()
        Type::Tuple(TypeTuple { elems, .. }) if elems.is_empty() => Ok(TypeRef::unit()),

        // Handle other types as references
        _ => Err(NormalizeError::TypePathError(
            "unsupported type".to_string(),
        )),
    }
}

/// Check if a path represents Option<T>
fn is_option(path: &syn::Path) -> bool {
    path.segments.len() == 1 && path.segments[0].ident == "Option"
}

/// Extract the inner type from Option<T>
fn extract_option_inner(ty: &Type) -> Result<&Type, NormalizeError> {
    if let Type::Path(TypePath { path, .. }) = ty {
        if let Some(segment) = path.segments.first() {
            if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                if let Some(syn::GenericArgument::Type(inner_type)) = args.args.first() {
                    return Ok(inner_type);
                }
            }
        }
    }
    Err(NormalizeError::TypePathError(
        "invalid Option type".to_string(),
    ))
}

/// Check if a path represents Vec<T>
fn is_vec(path: &syn::Path) -> bool {
    path.segments.len() == 1 && path.segments[0].ident == "Vec"
}

/// Check if a path represents Vec<u8>
fn is_vec_u8(path: &syn::Path) -> bool {
    if !is_vec(path) {
        return false;
    }
    if let Some(segment) = path.segments.first() {
        if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
            if let Some(syn::GenericArgument::Type(Type::Path(TypePath {
                path: inner_path, ..
            }))) = args.args.first()
            {
                return is_u8_type(inner_path);
            }
        }
    }
    false
}

/// Extract the inner type from Vec<T>
fn extract_vec_inner(ty: &Type) -> Result<&Type, NormalizeError> {
    if let Type::Path(TypePath { path, .. }) = ty {
        if let Some(segment) = path.segments.first() {
            if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                if let Some(syn::GenericArgument::Type(inner_type)) = args.args.first() {
                    return Ok(inner_type);
                }
            }
        }
    }
    Err(NormalizeError::TypePathError(
        "invalid Vec type".to_string(),
    ))
}

/// Check if a path represents BTreeMap<K, V>
fn is_btree_map(path: &syn::Path) -> bool {
    path.segments.len() == 1 && path.segments[0].ident == "BTreeMap"
}

/// Extract key and value types from BTreeMap<K, V>
fn extract_map_inner(ty: &Type) -> Result<(&Type, &Type), NormalizeError> {
    if let Type::Path(TypePath { path, .. }) = ty {
        if let Some(segment) = path.segments.first() {
            if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                if args.args.len() >= 2 {
                    if let (
                        syn::GenericArgument::Type(key_type),
                        syn::GenericArgument::Type(value_type),
                    ) = (&args.args[0], &args.args[1])
                    {
                        return Ok((key_type, value_type));
                    }
                }
            }
        }
    }
    Err(NormalizeError::TypePathError(
        "invalid BTreeMap type".to_string(),
    ))
}

/// Check if a path represents u8
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
            .map_err(|_| NormalizeError::TypePathError("failed to parse array length".to_string()))
    } else {
        Err(NormalizeError::TypePathError(
            "array length must be a literal integer".to_string(),
        ))
    }
}

/// Check if a TypeRef represents a string type
fn is_string_type(type_ref: &TypeRef) -> bool {
    matches!(type_ref, TypeRef::Scalar(crate::schema::ScalarType::String))
}

/// Normalize a scalar type
fn normalize_scalar_type(
    path: &syn::Path,
    _wasm32: bool,
    _resolver: &dyn TypeResolver,
) -> Result<TypeRef, NormalizeError> {
    if path.segments.len() != 1 {
        return Err(NormalizeError::TypePathError(
            "invalid scalar type path".to_string(),
        ));
    }

    let ident = &path.segments[0].ident;
    match ident.to_string().as_str() {
        "bool" => Ok(TypeRef::bool()),
        "i32" => Ok(TypeRef::i32()),
        "i64" => Ok(TypeRef::i64()),
        "u32" => Ok(TypeRef::u32()),
        "u64" => Ok(TypeRef::u64()),
        "f32" => Ok(TypeRef::f32()),
        "f64" => Ok(TypeRef::f64()),
        "String" => Ok(TypeRef::string()),
        _ => {
            // Check if it's a local type
            if let Some(resolved) = _resolver.resolve_local(&ident.to_string()) {
                match resolved {
                    ResolvedLocal::NewtypeBytes { size } => {
                        Ok(TypeRef::bytes_with_size(size, "hex"))
                    }
                    ResolvedLocal::Record => Ok(TypeRef::reference(&ident.to_string())),
                    ResolvedLocal::Variant => Ok(TypeRef::reference(&ident.to_string())),
                }
            } else {
                Err(NormalizeError::TypePathError(format!(
                    "unknown type: {}",
                    ident
                )))
            }
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
