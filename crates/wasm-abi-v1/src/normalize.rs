use crate::schema::TypeRef;
use syn::{Type, TypePath, TypeReference, TypeArray, TypeTuple};
use thiserror::Error;

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
        Type::Reference(TypeReference { elem, .. }) => {
            normalize_type(elem, wasm32, resolver)
        }

        // Handle Option<T>
        Type::Path(TypePath { path, .. }) if is_option(path) => {
            let inner_type = extract_option_inner(ty)?;
            let mut inner_ref = normalize_type(inner_type, wasm32, resolver)?;
            inner_ref.set_nullable(true);
            Ok(inner_ref)
        }

        // Handle Vec<u8> (bytes without size) - must come before generic Vec<T>
        Type::Path(TypePath { path, .. }) if is_vec_u8(path) => {
            Ok(TypeRef::bytes())
        }

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
                    format!("non-string key type")
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
                format!("non-u8 element type")
            ))
        }

        // Handle scalar types
        Type::Path(TypePath { path, .. }) => {
            normalize_scalar_type(path, wasm32, resolver)
        }

        // Handle unit type ()
        Type::Tuple(TypeTuple { elems, .. }) if elems.is_empty() => {
            Ok(TypeRef::unit())
        }

        // Handle other types as references
        _ => {
            Err(NormalizeError::TypePathError(
                format!("unsupported type")
            ))
        }
    }
}

/// Check if a type path represents Option
fn is_option(path: &syn::Path) -> bool {
    path.segments.last()
        .map(|seg| seg.ident == "Option")
        .unwrap_or(false)
}

/// Check if a type path represents Vec
fn is_vec(path: &syn::Path) -> bool {
    path.segments.last()
        .map(|seg| seg.ident == "Vec")
        .unwrap_or(false)
}

/// Check if a type path represents BTreeMap
fn is_btree_map(path: &syn::Path) -> bool {
    path.segments.last()
        .map(|seg| seg.ident == "BTreeMap")
        .unwrap_or(false)
}

/// Check if a type path represents u8
fn is_u8_type(path: &syn::Path) -> bool {
    path.segments.last()
        .map(|seg| seg.ident == "u8")
        .unwrap_or(false)
}

/// Check if a type path represents Vec<u8>
fn is_vec_u8(path: &syn::Path) -> bool {
    if let Some(last_seg) = path.segments.last() {
        if last_seg.ident == "Vec" {
            if let syn::PathArguments::AngleBracketed(args) = &last_seg.arguments {
                if args.args.len() == 1 {
                    if let syn::GenericArgument::Type(Type::Path(TypePath { path: inner_path, .. })) = &args.args[0] {
                        return is_u8_type(inner_path);
                    }
                }
            }
        }
    }
    false
}

/// Extract the inner type from Option<T>
fn extract_option_inner(ty: &syn::Type) -> Result<&syn::Type, NormalizeError> {
    if let Type::Path(TypePath { path, .. }) = ty {
        if let Some(last_seg) = path.segments.last() {
            if let syn::PathArguments::AngleBracketed(args) = &last_seg.arguments {
                if args.args.len() == 1 {
                    if let syn::GenericArgument::Type(inner_type) = &args.args[0] {
                        return Ok(inner_type);
                    }
                }
            }
        }
    }
    Err(NormalizeError::TypePathError("failed to extract Option inner type".to_string()))
}

/// Extract the inner type from Vec<T>
fn extract_vec_inner(ty: &syn::Type) -> Result<&syn::Type, NormalizeError> {
    if let Type::Path(TypePath { path, .. }) = ty {
        if let Some(last_seg) = path.segments.last() {
            if let syn::PathArguments::AngleBracketed(args) = &last_seg.arguments {
                if args.args.len() == 1 {
                    if let syn::GenericArgument::Type(inner_type) = &args.args[0] {
                        return Ok(inner_type);
                    }
                }
            }
        }
    }
    Err(NormalizeError::TypePathError("failed to extract Vec inner type".to_string()))
}

/// Extract key and value types from BTreeMap<K, V>
fn extract_map_inner(ty: &syn::Type) -> Result<(&syn::Type, &syn::Type), NormalizeError> {
    if let Type::Path(TypePath { path, .. }) = ty {
        if let Some(last_seg) = path.segments.last() {
            if let syn::PathArguments::AngleBracketed(args) = &last_seg.arguments {
                if args.args.len() == 2 {
                    if let (syn::GenericArgument::Type(key_type), syn::GenericArgument::Type(value_type)) = 
                        (&args.args[0], &args.args[1]) {
                        return Ok((key_type, value_type));
                    }
                }
            }
        }
    }
    Err(NormalizeError::TypePathError("failed to extract BTreeMap inner types".to_string()))
}

/// Extract array length from [T; N]
fn extract_array_len(len: &syn::Expr) -> Result<usize, NormalizeError> {
    if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Int(lit), .. }) = len {
        lit.base10_parse().map_err(|_| {
            NormalizeError::TypePathError("failed to parse array length".to_string())
        })
    } else {
        Err(NormalizeError::TypePathError("array length must be a literal integer".to_string()))
    }
}

/// Check if a TypeRef represents a string type
fn is_string_type(ty: &TypeRef) -> bool {
    matches!(ty, TypeRef::Scalar(crate::schema::ScalarType::String))
}

/// Normalize scalar types and named types
fn normalize_scalar_type(
    path: &syn::Path,
    wasm32: bool,
    resolver: &dyn TypeResolver,
) -> Result<TypeRef, NormalizeError> {
    if let Some(last_seg) = path.segments.last() {
        let type_name = last_seg.ident.to_string();
        
        match type_name.as_str() {
            "bool" => Ok(TypeRef::bool()),
            "i32" => Ok(TypeRef::i32()),
            "i64" => Ok(TypeRef::i64()),
            "u32" => Ok(TypeRef::u32()),
            "u64" => Ok(TypeRef::u64()),
            "f32" => Ok(TypeRef::f32()),
            "f64" => Ok(TypeRef::f64()),
            "String" => Ok(TypeRef::string()),
            "str" => Ok(TypeRef::string()),
            "usize" if wasm32 => Ok(TypeRef::u32()),
            "isize" if wasm32 => Ok(TypeRef::i32()),
            "usize" => Ok(TypeRef::u64()),
            "isize" => Ok(TypeRef::i64()),
            _ => {
                // Check if it's a local type that needs resolution
                if let Some(resolved) = resolver.resolve_local(&type_name) {
                    match resolved {
                        ResolvedLocal::NewtypeBytes { size: _ } => {
                            // Return a reference to the type name, upstream will handle the bytes definition
                            Ok(TypeRef::reference(&type_name))
                        }
                        ResolvedLocal::Record | ResolvedLocal::Variant => {
                            Ok(TypeRef::reference(&type_name))
                        }
                    }
                } else {
                    // Unknown external type, return as reference
                    Ok(TypeRef::reference(&type_name))
                }
            }
        }
    } else {
        Err(NormalizeError::TypePathError("empty type path".to_string()))
    }
}

// Extension trait to add nullable support to TypeRef
trait TypeRefExt {
    fn set_nullable(&mut self, nullable: bool);
}

impl TypeRefExt for TypeRef {
    fn set_nullable(&mut self, _nullable: bool) {
        // For now, we'll store nullable as a field in the schema
        // This will be handled by the higher-level emitter
        // The actual nullable flag is set on the Parameter/Field level
    }
} 