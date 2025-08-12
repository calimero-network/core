use proc_macro2::TokenStream;
use quote::quote;
use syn::{Type, PathArguments, GenericArgument, Item, ItemStruct, ItemEnum, Fields, Error as SynError};
use calimero_wasm_abi_v1::{Manifest, Method, Parameter, TypeRef, TypeDef, Field as AbiField, Variant as AbiVariant};

use crate::logic::method::PublicLogicMethod;
use std::collections::HashMap;
use std::cell::RefCell;

// Global registry for ABI type definitions
thread_local! {
    static ABI_TYPE_REGISTRY: RefCell<HashMap<String, TypeDef>> = RefCell::new(HashMap::new());
}

/// Register a type definition for ABI expansion
pub fn register_abi_type(item: &Item) -> Result<TokenStream, SynError> {
    match item {
        Item::Struct(item_struct) => {
            let type_name = item_struct.ident.to_string();
            let type_def = convert_struct_to_type_def(item_struct)?;
            
            ABI_TYPE_REGISTRY.with(|registry| {
                let _ = registry.borrow_mut().insert(type_name, type_def);
            });
            
            // Return the original item unchanged
            Ok(quote! { #item })
        }
        Item::Enum(item_enum) => {
            let type_name = item_enum.ident.to_string();
            let type_def = convert_enum_to_type_def(item_enum)?;
            
            ABI_TYPE_REGISTRY.with(|registry| {
                let _ = registry.borrow_mut().insert(type_name, type_def);
            });
            
            // Return the original item unchanged
            Ok(quote! { #item })
        }
        _ => {
            Err(SynError::new_spanned(
                item,
                "abi_type macro can only be used on structs and enums",
            ))
        }
    }
}

/// Get all registered ABI types
pub fn get_registered_types() -> HashMap<String, TypeDef> {
    ABI_TYPE_REGISTRY.with(|registry| {
        registry.borrow().clone()
    })
}

/// Convert a Rust struct to ABI TypeDef
fn convert_struct_to_type_def(item_struct: &ItemStruct) -> Result<TypeDef, SynError> {
    let mut fields = Vec::new();
    
    match &item_struct.fields {
        Fields::Named(named_fields) => {
            for field in &named_fields.named {
                if let Some(ident) = &field.ident {
                    let field_type = normalize_type(&field.ty);
                    let nullable = is_option_type(&field.ty);
                    
                    fields.push(AbiField {
                        name: ident.to_string(),
                        type_: field_type,
                        nullable: if nullable { Some(true) } else { None },
                    });
                }
            }
        }
        Fields::Unnamed(_) => {
            return Err(SynError::new_spanned(
                &item_struct.fields,
                "tuple structs are not supported in ABI type definitions",
            ));
        }
        Fields::Unit => {
            // Unit structs - no fields
        }
    }
    
    Ok(TypeDef::Record { fields })
}

/// Convert a Rust enum to ABI TypeDef
fn convert_enum_to_type_def(item_enum: &ItemEnum) -> Result<TypeDef, SynError> {
    let mut variants = Vec::new();
    
    for variant in &item_enum.variants {
        let variant_type = match &variant.fields {
            Fields::Named(_) => {
                // For now, treat named fields as a generic record
                Some(TypeRef::string())
            }
            Fields::Unnamed(fields) => {
                if fields.unnamed.len() == 1 {
                    Some(normalize_type(&fields.unnamed[0].ty))
                } else {
                    // Multiple unnamed fields - treat as generic
                    Some(TypeRef::string())
                }
            }
            Fields::Unit => None,
        };
        
        variants.push(AbiVariant {
            name: variant.ident.to_string(),
            type_: variant_type,
        });
    }
    
    Ok(TypeDef::Variant { variants })
}

/// Check if a type is Option<T>
fn is_option_type(ty: &Type) -> bool {
    if let Type::Path(type_path) = ty {
        if let Some(ident) = type_path.path.get_ident() {
            return ident.to_string() == "Option";
        }
    }
    false
}

/// Generate ABI manifest from public methods
pub fn generate_abi(methods: &[PublicLogicMethod<'_>], _type_definitions: &[()]) -> TokenStream {
    let mut manifest = Manifest::default();
    
    // Collect all methods
    for method in methods {
        let method_def = collect_method(method);
        manifest.methods.push(method_def);
    }
    
    // Add registered type definitions to the manifest
    let registered_types = get_registered_types();
    for (type_name, type_def) in registered_types {
        let _ = manifest.types.insert(type_name, type_def);
    }
    
    // Generate the embed code directly
    let json = serde_json::to_string_pretty(&manifest)
        .expect("Failed to serialize manifest to JSON");
    
    let json_bytes = json.as_bytes();
    let byte_array_literal = proc_macro2::Literal::byte_string(json_bytes);
    let len_literal = proc_macro2::Literal::usize_unsuffixed(json_bytes.len());
    
    quote! {
        // Embed ABI manifest
        #[link_section = "calimero_abi_v1"]
        static ABI: [u8; #len_literal] = *#byte_array_literal;
        
        #[no_mangle]
        pub extern "C" fn get_abi_ptr() -> u32 {
            ABI.as_ptr() as u32
        }
        
        #[no_mangle]
        pub extern "C" fn get_abi_len() -> u32 {
            ABI.len() as u32
        }
        
        #[no_mangle]
        pub extern "C" fn get_abi() -> u32 {
            get_abi_ptr()
        }
    }
}

/// Collect method information for ABI
fn collect_method(method: &PublicLogicMethod<'_>) -> Method {
    let mut params = Vec::new();
    
    // Convert arguments to parameters
    for arg in &method.args {
        let param = Parameter {
            name: arg.ident.to_string(),
            type_: normalize_type(&arg.ty.ty),
            nullable: None, // Will be set by normalize_type if needed
        };
        params.push(param);
    }
    
    // Handle return type
    let returns = method.ret.as_ref().map(|ret| normalize_type(&ret.ty));
    
    Method {
        name: method.name.to_string(),
        params,
        returns,
        errors: Vec::new(), // TODO: Extract from Result<T, E> types
    }
}

/// Normalize Rust types to WASM-compatible ABI types
fn normalize_type(ty: &Type) -> TypeRef {
    match ty {
        Type::Path(path) => {
            if let Some(ident) = path.path.get_ident() {
                match ident.to_string().as_str() {
                    "bool" => TypeRef::bool(),
                    "i32" => TypeRef::i32(),
                    "i64" => TypeRef::i64(),
                    "u32" => TypeRef::u32(),
                    "u64" => TypeRef::u64(),
                    "f32" => TypeRef::f32(),
                    "f64" => TypeRef::f64(),
                    "usize" => TypeRef::u32(), // wasm32
                    "isize" => TypeRef::i32(), // wasm32
                    "String" => TypeRef::string(),
                    "str" => TypeRef::string(),
                    "Vec" => {
                        // Handle Vec<T>
                        if let Some(segment) = path.path.segments.last() {
                            if let PathArguments::AngleBracketed(args) = &segment.arguments {
                                if let Some(arg) = args.args.first() {
                                    if let GenericArgument::Type(item_type) = arg {
                                        let inner_type = normalize_type(item_type);
                                        return TypeRef::list(inner_type);
                                    }
                                }
                            }
                        }
                        TypeRef::list(TypeRef::string()) // fallback
                    }
                    "Option" => {
                        // Handle Option<T>
                        if let Some(segment) = path.path.segments.last() {
                            if let PathArguments::AngleBracketed(args) = &segment.arguments {
                                if let Some(arg) = args.args.first() {
                                    if let GenericArgument::Type(item_type) = arg {
                                        let inner_type = normalize_type(item_type);
                                        // Mark as nullable
                                        return inner_type;
                                    }
                                }
                            }
                        }
                        TypeRef::string() // fallback
                    }
                    "Result" => {
                        // Handle Result<T, E> - extract T as return type
                        if let Some(segment) = path.path.segments.last() {
                            if let PathArguments::AngleBracketed(args) = &segment.arguments {
                                if let Some(arg) = args.args.first() {
                                    if let GenericArgument::Type(item_type) = arg {
                                        return normalize_type(item_type);
                                    }
                                }
                            }
                        }
                        TypeRef::string() // fallback
                    }
                    _ => {
                        // For custom types, create a reference
                        TypeRef::reference(&ident.to_string())
                    }
                }
            } else {
                TypeRef::string() // fallback
            }
        }
        Type::Reference(ref_) => {
            // Strip reference and normalize inner type
            normalize_type(&ref_.elem)
        }
        Type::Slice(slice) => {
            // Handle [T] as Vec<T>
            let inner_type = normalize_type(&slice.elem);
            TypeRef::list(inner_type)
        }
        Type::Array(array) => {
            // Handle [T; N] as Vec<T>
            let inner_type = normalize_type(&array.elem);
            TypeRef::list(inner_type)
        }
        _ => TypeRef::string(), // fallback for unknown types
    }
}

 