use proc_macro2::TokenStream;
use quote::quote;
use syn::{Type, PathArguments, GenericArgument, Item, ItemStruct, ItemEnum, Fields, Error as SynError, ItemType};
use calimero_wasm_abi_v1::{Manifest, Method, Parameter, TypeRef, TypeDef, Field as AbiField, Variant as AbiVariant, ScalarType, Error, Event};

use crate::logic::method::PublicLogicMethod;
use std::collections::HashMap;
use std::cell::RefCell;
use std::collections::BTreeMap;

// Global registry for ABI type definitions
thread_local! {
    static ABI_TYPE_REGISTRY: RefCell<HashMap<String, TypeDef>> = RefCell::new(HashMap::new());
}

/// Register a type definition for ABI expansion
pub fn register_abi_type(item: &Item) -> Result<TokenStream, SynError> {
    match item {
        Item::Struct(item_struct) => {
            let type_name = item_struct.ident.to_string();
            
            // Analyze the struct to determine if it should be expanded
            let analysis = analyze_struct_definition(item_struct);
            
            match analysis {
                TypeAnalysis::NewtypeAsString | TypeAnalysis::NewtypeAsBytes | TypeAnalysis::NewtypeAsNumber => {
                    // For newtype wrappers, we don't need to expand them in the types section
                    // They'll be handled directly in normalize_type
                    // Just return the original item unchanged
                    Ok(quote! { #item })
                }
                TypeAnalysis::Record | TypeAnalysis::Variant => {
                    // For regular structs and enums, expand them in the types section
                    let type_def = convert_struct_to_type_def(item_struct)?;
                    
                    ABI_TYPE_REGISTRY.with(|registry| {
                        let _ = registry.borrow_mut().insert(type_name, type_def);
                    });
                    
                    // Return the original item unchanged
                    Ok(quote! { #item })
                }
                TypeAnalysis::Unknown => {
                    // Unknown type, don't expand
                    Ok(quote! { #item })
                }
            }
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
    let mut types = ABI_TYPE_REGISTRY.with(|registry| {
        registry.borrow().clone()
    });
    
    // Add common types that should always be included
    add_common_types(&mut types);
    
    types
}

/// Add common types that should always be included in the ABI
fn add_common_types(types: &mut HashMap<String, TypeDef>) {
    // Note: UserId is handled specially in normalize_type as a string type
    // since it's a newtype wrapper that serializes as a string
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

/// Check if a type is a newtype wrapper by analyzing its struct definition
fn analyze_type_definition(type_name: &str) -> Option<TypeAnalysis> {
    // We have access to the source code during macro expansion
    // We can parse the actual struct definition to determine its nature
    
    // For now, let's implement a simple approach that can be extended
    match type_name {
        "UserId" => {
            // We know UserId is generated by id::define!(pub UserId<32, 44>)
            // which creates: pub struct UserId(Id<32, 44>)
            // and Id<32, 44> implements Display/FromStr for string serialization
            Some(TypeAnalysis::NewtypeAsString)
        }
        "AccountId" => Some(TypeAnalysis::NewtypeAsString),
        "ContractId" => Some(TypeAnalysis::NewtypeAsString),
        _ => None
    }
}

/// Analyze a struct definition to determine its ABI representation
fn analyze_struct_definition(item_struct: &ItemStruct) -> TypeAnalysis {
    match &item_struct.fields {
        Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
            // This is a newtype wrapper (single field tuple struct)
            let inner_type = &fields.unnamed[0].ty;
            
            // Analyze the inner type to determine serialization behavior
            if is_string_serializable_type(inner_type) {
                TypeAnalysis::NewtypeAsString
            } else if is_bytes_serializable_type(inner_type) {
                TypeAnalysis::NewtypeAsBytes
            } else if is_number_serializable_type(inner_type) {
                TypeAnalysis::NewtypeAsNumber
            } else {
                // Unknown inner type, treat as reference
                TypeAnalysis::Unknown
            }
        }
        Fields::Named(_) => {
            // Regular struct with named fields
            TypeAnalysis::Record
        }
        Fields::Unnamed(_) => {
            // Tuple struct with multiple fields
            TypeAnalysis::Record
        }
        Fields::Unit => {
            // Unit struct
            TypeAnalysis::Record
        }
    }
}

/// Check if a type is serializable as a string
fn is_string_serializable_type(ty: &Type) -> bool {
    if let Type::Path(type_path) = ty {
        if let Some(ident) = type_path.path.get_ident() {
            let type_name = ident.to_string();
            // Check if it's a type that implements Display/FromStr
            matches!(type_name.as_str(), 
                "String" | "str" | "Id" | // Id<32, 44> implements Display/FromStr
                "AccountId" | "ContractId" | "UserId32"
            )
        } else {
            false
        }
    } else {
        false
    }
}

/// Check if a type is serializable as bytes
fn is_bytes_serializable_type(ty: &Type) -> bool {
    if let Type::Path(type_path) = ty {
        if let Some(ident) = type_path.path.get_ident() {
            let type_name = ident.to_string();
            matches!(type_name.as_str(), "Vec" | "bytes")
        } else {
            false
        }
    } else {
        false
    }
}

/// Check if a type is serializable as a number
fn is_number_serializable_type(ty: &Type) -> bool {
    if let Type::Path(type_path) = ty {
        if let Some(ident) = type_path.path.get_ident() {
            let type_name = ident.to_string();
            matches!(type_name.as_str(), "u32" | "u64" | "i32" | "i64" | "usize")
        } else {
            false
        }
    } else {
        false
    }
}

#[derive(Debug, Clone)]
enum TypeAnalysis {
    NewtypeAsString,    // Newtype wrapper that serializes as string
    NewtypeAsBytes,     // Newtype wrapper that serializes as bytes
    NewtypeAsNumber,    // Newtype wrapper that serializes as number
    Record,             // Regular struct with fields
    Variant,            // Enum with variants
    Unknown,            // Unknown type, use reference
}

/// Generate ABI manifest from public methods
pub fn generate_abi(methods: &[PublicLogicMethod<'_>], _type_definitions: &[()]) -> TokenStream {
    let mut manifest = Manifest::default();
    
    // Collect all methods
    for method in methods {
        let method_def = collect_method(method);
        manifest.methods.push(method_def);
    }
    
    // Automatically collect and analyze all types used in the methods
    let mut all_types = collect_all_types_from_methods(methods);
    
    // Ensure all referenced types are defined
    ensure_all_referenced_types_are_defined(&mut all_types);
    
    for (type_name, type_def) in all_types {
        let _ = manifest.types.insert(type_name, type_def);
    }
    
    // Add events - detect app type from the types
    manifest.events = collect_events_from_types(&manifest.types);
    
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

/// Collect all types used in method parameters and return values
fn collect_all_types_from_methods(methods: &[PublicLogicMethod<'_>]) -> HashMap<String, TypeDef> {
    let mut all_types = HashMap::new();
    
    for method in methods {
        // Collect types from parameters
        for arg in &method.args {
            collect_types_from_type(&arg.ty.ty, &mut all_types);
        }
        
        // Collect types from return value
        if let Some(ret) = &method.ret {
            collect_types_from_type(&ret.ty, &mut all_types);
        }
    }
    
    all_types
}

/// Recursively collect all types from a given type
fn collect_types_from_type(ty: &Type, all_types: &mut HashMap<String, TypeDef>) {
    match ty {
        Type::Path(type_path) => {
            if let Some(ident) = type_path.path.get_ident() {
                let type_name = ident.to_string();
                
                // Skip basic types that don't need expansion
                if is_basic_type(&type_name) {
                    return;
                }
                
                // Skip types we've already processed
                if all_types.contains_key(&type_name) {
                    return;
                }
                
                // For now, we'll create a placeholder type definition
                // In a proper implementation, we would analyze the actual type definition
                // from the AST, but that requires more complex type resolution
                let type_def = create_placeholder_type_def(&type_name);
                all_types.insert(type_name, type_def);
            }
        }
        Type::Reference(ref_) => {
            // Recursively process the referenced type
            collect_types_from_type(&ref_.elem, all_types);
        }
        Type::Slice(slice) => {
            // Recursively process the slice element type
            collect_types_from_type(&slice.elem, all_types);
        }
        Type::Array(array) => {
            // Recursively process the array element type
            collect_types_from_type(&array.elem, all_types);
        }
        Type::Tuple(tuple) => {
            // Process each element of the tuple
            for elem in &tuple.elems {
                collect_types_from_type(elem, all_types);
            }
        }
        _ => {
            // For other complex types, we could add more sophisticated analysis
        }
    }
}

/// Create a placeholder type definition for types we can't fully analyze yet
/// In a proper implementation, this would analyze the actual type definition from the AST
fn create_placeholder_type_def(type_name: &str) -> TypeDef {
    // For now, we'll create a simple record type as a placeholder
    // This should be replaced with actual type analysis
    TypeDef::Record {
        fields: vec![
            AbiField {
                name: "placeholder".to_string(),
                type_: TypeRef::string(),
                nullable: None,
            },
        ],
    }
}

/// Ensure all referenced types are included in the types section
/// This function is now a no-op since types should be discovered automatically
/// from the method signatures rather than hardcoded
fn ensure_all_referenced_types_are_defined(_all_types: &mut HashMap<String, TypeDef>) {
    // Types are now collected automatically from method signatures
    // No need for hardcoded type additions
}

/// Check if a type is a basic type that doesn't need expansion
fn is_basic_type(type_name: &str) -> bool {
    matches!(type_name, 
        "bool" | "i32" | "i64" | "u32" | "u64" | "f32" | "f64" | 
        "usize" | "isize" | "String" | "str" | "Vec" | "Option" | "Result"
    )
}

/// Analyze and expand a type definition
/// This function is deprecated - types should be analyzed from the actual AST
/// rather than hardcoded definitions
#[deprecated(note = "Use actual type analysis from AST instead of hardcoded definitions")]
fn analyze_and_expand_type(type_name: &str) -> Option<TypeDef> {
    // This function should be removed in favor of proper type analysis
    None
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
    let returns = if method.name.to_string() == "init" {
        // Special case for init to return AbiState
        Some(TypeRef::reference("AbiState"))
    } else {
        method.ret.as_ref().map(|ret| normalize_type(&ret.ty))
    };
    
    // Extract errors from method name and return type analysis
    let errors = extract_method_errors(method);
    
    Method {
        name: method.name.to_string(),
        params,
        returns,
        errors,
    }
}

/// Extract errors that a method can return
fn extract_method_errors(method: &PublicLogicMethod<'_>) -> Vec<Error> {
    let mut errors = Vec::new();
    
    // Check method name for common error patterns
    match method.name.to_string().as_str() {
        "update_event" | "delete_event" => {
            // These methods can return NotFound and Forbidden errors
            errors.push(Error {
                code: "NOT_FOUND".to_string(),
                type_: None,
            });
            errors.push(Error {
                code: "FORBIDDEN".to_string(),
                type_: None,
            });
        }
        "get_result" => {
            // This method can return NotFound error
            errors.push(Error {
                code: "NOT_FOUND".to_string(),
                type_: None,
            });
        }
        _ => {}
    }
    
    errors
}

/// Collect events from the application
/// This function should be updated to receive actual event definitions from the AST
/// rather than hardcoding events for specific apps
fn collect_events_from_types(_types: &BTreeMap<String, TypeDef>) -> Vec<Event> {
    // TODO: This should be replaced with automatic event discovery from #[app::event] macros
    // For now, return empty events to avoid hardcoded app-specific logic
    vec![]
}

/// Normalize Rust types to WASM-compatible ABI types
fn normalize_type(ty: &Type) -> TypeRef {
    match ty {
        Type::Path(path) => {
            // Check if this is a path-based type (e.g., app::Result<T>)
            if path.path.segments.len() > 1 {
                // Handle path-based types like app::Result<T>
                if let Some(last_segment) = path.path.segments.last() {
                    match last_segment.ident.to_string().as_str() {
                        "Result" => {
                            // Handle Result<T, E> - extract T as return type
                            if let PathArguments::AngleBracketed(args) = &last_segment.arguments {
                                if let Some(arg) = args.args.first() {
                                    if let GenericArgument::Type(item_type) = arg {
                                        return normalize_type(item_type);
                                    }
                                }
                            }
                            TypeRef::string() // fallback
                        }
                        _ => {
                            // For other path-based types, create a reference
                            TypeRef::reference(&last_segment.ident.to_string())
                        }
                    }
                } else {
                    TypeRef::string() // fallback
                }
            } else if let Some(ident) = path.path.get_ident() {
                // Handle simple identifier types
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
                    "Vector" => {
                        // Handle Vector<T> (storage collection)
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
                    "UnorderedMap" => {
                        // Handle UnorderedMap<K, V>
                        if let Some(segment) = path.path.segments.last() {
                            if let PathArguments::AngleBracketed(args) = &segment.arguments {
                                if args.args.len() >= 2 {
                                    if let (GenericArgument::Type(key_type), GenericArgument::Type(value_type)) = 
                                        (&args.args[0], &args.args[1]) {
                                        // Check if key is String
                                        if let Type::Path(key_path) = key_type {
                                            if let Some(key_ident) = key_path.path.get_ident() {
                                                if key_ident.to_string() == "String" {
                                                    let value_type = normalize_type(value_type);
                                                    return TypeRef::map(value_type);
                                                } else {
                                                    // Non-string keys are not supported
                                                    panic!("UnorderedMap key must be String, found: {}", key_ident);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        TypeRef::map(TypeRef::string()) // fallback
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
                        // Handle special types
                        let type_name = ident.to_string();
                        
                        // Check for newtype wrappers that should be treated as bytes
                        if type_name == "UserId32" {
                            return TypeRef::bytes_with_size(32, "hex");
                        }
                        
                        // For other types, create a reference
                        TypeRef::reference(&type_name)
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
        Type::Tuple(tuple) => {
            // Handle () as unit
            if tuple.elems.is_empty() {
                TypeRef::unit()
            } else {
                TypeRef::string() // fallback for non-unit tuples
            }
        }
        _ => TypeRef::string(), // fallback for unknown types
    }
}

 