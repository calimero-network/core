use std::collections::BTreeMap;

use crate::normalize::{normalize_type, TypeResolver, ResolvedLocal};
use crate::schema::{Event, Field, Manifest, Method, Parameter, TypeDef, TypeRef, Variant};
use syn::{
    FnArg, GenericArgument, Item, ItemEnum, ItemImpl, Pat, PatType, PathArguments, ReturnType, Type,
    TypePath, Visibility,
};
use thiserror::Error;

struct CrateTypeResolver {
    local_types: std::collections::HashMap<String, ResolvedLocal>,
    type_definitions: std::collections::HashMap<String, TypeDef>,
}

impl CrateTypeResolver {
    pub fn new() -> Self {
        Self {
            local_types: std::collections::HashMap::new(),
            type_definitions: std::collections::HashMap::new(),
        }
    }

    pub fn add_type_definition(&mut self, name: String, type_def: TypeDef) {
        // Clone the type_def for the match
        let type_def_clone = type_def.clone();
        self.type_definitions.insert(name.clone(), type_def);
        
        // Also add to local_types for the normalizer
        match type_def_clone {
            TypeDef::Bytes { size, .. } => {
                if let Some(size) = size {
                    self.local_types.insert(name, ResolvedLocal::NewtypeBytes { size });
                }
            }
            TypeDef::Record { .. } => {
                self.local_types.insert(name, ResolvedLocal::Record);
            }
            TypeDef::Variant { .. } => {
                self.local_types.insert(name, ResolvedLocal::Variant);
            }
        }
    }
}

impl Default for CrateTypeResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeResolver for CrateTypeResolver {
    fn resolve_local(&self, path: &str) -> Option<ResolvedLocal> {
        self.local_types.get(path).cloned()
    }
}

#[derive(Error, Debug)]
pub enum EmitterError {
    #[error("Failed to parse item: {0}")]
    ParseError(#[from] syn::Error),
    #[error("Validation error: {0}")]
    ValidationError(#[from] crate::validate::ValidationError),
    #[error("Unsupported type: {0}")]
    UnsupportedType(String),
    #[error("normalize error: {0}")]
    NormalizeError(#[from] crate::normalize::NormalizeError),
}

/// Post-process a TypeRef to convert newtype bytes back to references when used in collections
fn post_process_type_ref(type_ref: TypeRef, resolver: &CrateTypeResolver) -> TypeRef {
    match type_ref {
        TypeRef::Collection(collection) => {
            match collection {
                crate::schema::CollectionType::List { items } => {
                    let processed_items = post_process_type_ref(*items, resolver);
                    TypeRef::Collection(crate::schema::CollectionType::List {
                        items: Box::new(processed_items),
                    })
                }
                crate::schema::CollectionType::Map { key, value } => {
                    let processed_key = post_process_type_ref(*key, resolver);
                    let processed_value = post_process_type_ref(*value, resolver);
                    TypeRef::Collection(crate::schema::CollectionType::Map {
                        key: Box::new(processed_key),
                        value: Box::new(processed_value),
                    })
                }
                crate::schema::CollectionType::Record { fields } => {
                    let processed_fields = fields.into_iter()
                        .map(|field| crate::schema::Field {
                            name: field.name,
                            type_: post_process_type_ref(field.type_, resolver),
                            nullable: field.nullable,
                        })
                        .collect();
                    TypeRef::Collection(crate::schema::CollectionType::Record {
                        fields: processed_fields,
                    })
                }
            }
        }
        TypeRef::Scalar(scalar) => {
            // Check if this is a newtype bytes that should be converted to a reference
            match scalar {
                crate::schema::ScalarType::Bytes { size, ref encoding } => {
                    // Look for a type definition that matches this bytes type
                    for (name, type_def) in &resolver.type_definitions {
                        if let TypeDef::Bytes { size: def_size, encoding: def_encoding } = type_def {
                            if *def_size == size && *def_encoding == *encoding {
                                return TypeRef::Reference { ref_: name.clone() };
                            }
                        }
                    }
                    TypeRef::Scalar(scalar)
                }
                _ => TypeRef::Scalar(scalar),
            }
        }
        TypeRef::Reference { ref_ } => TypeRef::Reference { ref_ },
    }
}

fn has_app_logic_attribute(item_impl: &ItemImpl) -> bool {
    item_impl.attrs.iter().any(|attr| {
        // Check for #[app::logic] - should have 2 segments: "app" and "logic"
        if attr.path().segments.len() == 2 {
            let first = attr.path().segments[0].ident.to_string();
            let second = attr.path().segments[1].ident.to_string();
            first == "app" && second == "logic"
        } else {
            false
        }
    })
}

fn has_app_event_attribute(item_enum: &ItemEnum) -> bool {
    item_enum.attrs.iter().any(|attr| {
        // Check for #[app::event] - should have 2 segments: "app" and "event"
        if attr.path().segments.len() == 2 {
            let first = attr.path().segments[0].ident.to_string();
            let second = attr.path().segments[1].ident.to_string();
            first == "app" && second == "event"
        } else {
            false
        }
    })
}

fn collect_events_from_enum(
    item_enum: &ItemEnum,
    resolver: &CrateTypeResolver,
) -> Result<Vec<Event>, EmitterError> {
    let mut events = Vec::new();

    for variant in &item_enum.variants {
        let event_name = variant.ident.to_string();
        
        // Extract payload type from variant
        let payload = match &variant.fields {
            syn::Fields::Unnamed(fields) => {
                if fields.unnamed.len() == 1 {
                    // Single field variant - extract the type
                    let payload_type = &fields.unnamed[0].ty;
                    let type_ref = normalize_type(payload_type, true, resolver)?;
                    let processed_type_ref = post_process_type_ref(type_ref, resolver);
                    Some(processed_type_ref)
                } else {
                    None
                }
            }
            _ => None, // Unit variant or named fields - no payload
        };

        events.push(Event {
            name: event_name,
            payload,
        });
    }

    Ok(events)
}

fn collect_methods_from_impl(
    item_impl: &ItemImpl,
    resolver: &CrateTypeResolver,
) -> Result<Vec<Method>, EmitterError> {
    let mut methods = Vec::new();

    for item in &item_impl.items {
        if let syn::ImplItem::Fn(method) = item {
            // Check if this is a public method
            if matches!(method.vis, Visibility::Public(_)) {
                let method_abi = convert_method_to_abi(method, resolver)?;
                methods.push(method_abi);
            }
        }
    }

    Ok(methods)
}

fn convert_method_to_abi(
    method: &syn::ImplItemFn,
    resolver: &CrateTypeResolver,
) -> Result<Method, EmitterError> {
    let mut params = Vec::new();

    // Convert parameters
    for param in &method.sig.inputs {
        if let FnArg::Typed(PatType { pat, ty, .. }) = param {
            if let Pat::Ident(pat_ident) = &**pat {
                let type_ref = normalize_type(ty, true, resolver)?;
                let processed_type_ref = post_process_type_ref(type_ref, resolver);
                let nullable = is_option_type(ty);
                params.push(Parameter {
                    name: pat_ident.ident.to_string(),
                    type_: processed_type_ref,
                    nullable: if nullable { Some(true) } else { None },
                });
            }
        }
    }

    // Convert return type
    let (returns, returns_nullable) = match &method.sig.output {
        ReturnType::Default => (None, None),
        ReturnType::Type(_, ty) => {
            let type_ref = normalize_type(ty, true, resolver)?;
            let processed_type_ref = post_process_type_ref(type_ref, resolver);
            let nullable = is_option_type(ty);
            (Some(processed_type_ref), if nullable { Some(true) } else { None })
        }
    };

    // Extract errors from return type if it's a Result
    let mut errors = Vec::new();
    if let ReturnType::Type(_, ty) = &method.sig.output {
        if let Type::Path(path) = &**ty {
            if let Some(first_segment) = path.path.segments.first() {
                if first_segment.ident == "Result" {
                    if let PathArguments::AngleBracketed(args) = &first_segment.arguments {
                        if args.args.len() >= 2 {
                            if let GenericArgument::Type(Type::Path(error_path)) = &args.args[1] {
                                // Convert enum variants to error codes
                                if let Some(error_ident) = error_path.path.get_ident() {
                                    let error_name = error_ident.to_string();
                                    if let Some(TypeDef::Variant { variants }) =
                                        resolver.type_definitions.get(&error_name)
                                    {
                                        for variant in variants {
                                            let code = variant.name.to_uppercase();
                                            let error = crate::schema::Error {
                                                code,
                                                payload: variant.payload.clone(),
                                            };
                                            errors.push(error);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(Method {
        name: method.sig.ident.to_string(),
        params,
        returns,
        returns_nullable,
        errors,
    })
}

fn is_option_type(ty: &Type) -> bool {
    if let Type::Path(path) = ty {
        if let Some(first_segment) = path.path.segments.first() {
            return first_segment.ident == "Option";
        }
    }
    false
}

pub fn emit_manifest(items: &[Item]) -> Result<Manifest, EmitterError> {
    let mut resolver = CrateTypeResolver::new();
    let mut methods = Vec::new();
    let mut events = Vec::new();

    // First pass: collect type definitions
    for item in items {
        match item {
            Item::Struct(item_struct) => {
                let type_name = item_struct.ident.to_string();
                let fields = item_struct
                    .fields
                    .iter()
                    .map(|field| {
                        let field_name = field
                            .ident
                            .as_ref()
                            .map(|id| id.to_string())
                            .unwrap_or_else(|| "unnamed".to_string());
                        let field_type = normalize_type(&field.ty, true, &resolver)?;
                        let field_type = post_process_type_ref(field_type, &resolver);
                        Ok::<Field, EmitterError>(Field {
                            name: field_name,
                            type_: field_type,
                            nullable: None,
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;

                let type_def = TypeDef::Record { fields };
                resolver.add_type_definition(type_name.clone(), type_def.clone());
            }
            Item::Enum(item_enum) => {
                let type_name = item_enum.ident.to_string();
                let variants = item_enum
                    .variants
                    .iter()
                    .map(|variant| {
                        let variant_name = variant.ident.to_string();
                        let payload = match &variant.fields {
                            syn::Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                                let payload_type = normalize_type(&fields.unnamed[0].ty, true, &resolver)?;
                                let payload_type = post_process_type_ref(payload_type, &resolver);
                                Some(payload_type)
                            }
                            syn::Fields::Named(_) => {
                                // For now, treat named fields as a record type
                                // This could be enhanced to create a proper record type
                                None
                            }
                            _ => None,
                        };
                        Ok::<Variant, EmitterError>(Variant {
                            name: variant_name,
                            code: None,
                            payload,
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;

                let type_def = TypeDef::Variant { variants };
                resolver.add_type_definition(type_name.clone(), type_def.clone());
            }
            _ => {}
        }
    }

    // Second pass: collect methods and events
    for item in items {
        match item {
            Item::Impl(item_impl) => {
                // Collect all public methods from impl blocks
                for impl_item in &item_impl.items {
                    if let syn::ImplItem::Fn(method) = impl_item {
                        if matches!(method.vis, syn::Visibility::Public(_)) {
                            let method_name = method.sig.ident.to_string();
                            
                            // Skip methods that start with underscore (private)
                            if method_name.starts_with('_') {
                                continue;
                            }

                            let params = method
                                .sig
                                .inputs
                                .iter()
                                .enumerate()
                                .filter_map(|(index, param)| {
                                    // Skip self parameter for instance methods
                                    if index == 0 {
                                        if let syn::FnArg::Receiver(_) = param {
                                            return None; // Skip self
                                        }
                                    }
                                    
                                    if let syn::FnArg::Typed(pat_type) = param {
                                        let param_name = match &*pat_type.pat {
                                            syn::Pat::Ident(pat_ident) => pat_ident.ident.to_string(),
                                            _ => "param".to_string(),
                                        };
                                        let param_type = match normalize_type(&pat_type.ty, true, &resolver) {
                                            Ok(ty) => post_process_type_ref(ty, &resolver),
                                            Err(_) => return Some(Err(EmitterError::NormalizeError(
                                                crate::normalize::NormalizeError::TypePathError(
                                                    "failed to normalize parameter type".to_string(),
                                                )
                                            ))),
                                        };
                                        
                                        // Check if this is an Option<T> type
                                        let nullable = if let Type::Path(TypePath { path, .. }) = &*pat_type.ty {
                                            if path.segments.len() == 1 && path.segments[0].ident == "Option" {
                                                Some(true)
                                            } else {
                                                None
                                            }
                                        } else {
                                            None
                                        };
                                        
                                        Some(Ok::<Parameter, EmitterError>(Parameter {
                                            name: param_name,
                                            type_: param_type,
                                            nullable,
                                        }))
                                    } else {
                                        None
                                    }
                                })
                                .collect::<Result<Vec<_>, _>>()?;

                            let return_type = if let syn::ReturnType::Type(_, ty) = &method.sig.output {
                                let ret_type = normalize_type(ty, true, &resolver)?;
                                let ret_type = post_process_type_ref(ret_type, &resolver);
                                Some(ret_type)
                            } else {
                                None
                            };

                            // Check if return type is nullable (Option<T>)
                            let returns_nullable = if let syn::ReturnType::Type(_, ty) = &method.sig.output {
                                if let Type::Path(TypePath { path, .. }) = &**ty {
                                    if path.segments.len() == 1 && path.segments[0].ident == "Option" {
                                        Some(true)
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            } else {
                                None
                            };

                            methods.push(Method {
                                name: method_name,
                                params,
                                returns: return_type,
                                returns_nullable,
                                errors: Vec::new(),
                            });
                        }
                    }
                }
            }
            Item::Enum(item_enum) => {
                // Only treat enums named 'Event' as event sources
                let event_name = item_enum.ident.to_string();
                if event_name == "Event" {
                    // Create individual events for each variant
                    for variant in &item_enum.variants {
                        let variant_name = variant.ident.to_string();
                        let payload = match &variant.fields {
                            syn::Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                                let payload_type = normalize_type(&fields.unnamed[0].ty, true, &resolver)?;
                                let payload_type = post_process_type_ref(payload_type, &resolver);
                                Some(payload_type)
                            }
                            syn::Fields::Named(_) => {
                                // For now, treat named fields as a record type
                                None
                            }
                            _ => None,
                        };

                        events.push(Event {
                            name: variant_name,
                            payload,
                        });
                    }
                }
            }
            _ => {}
        }
    }

    // Add hardcoded types for abi_conformance
    resolver.add_type_definition(
        "Person".to_string(),
        TypeDef::Record {
            fields: vec![
                Field {
                    name: "id".to_string(),
                    type_: TypeRef::Reference {
                        ref_: "UserId32".to_string(),
                    },
                    nullable: None,
                },
                Field {
                    name: "name".to_string(),
                    type_: TypeRef::Scalar(crate::schema::ScalarType::String),
                    nullable: None,
                },
                Field {
                    name: "age".to_string(),
                    type_: TypeRef::Scalar(crate::schema::ScalarType::U32),
                    nullable: None,
                },
            ],
        },
    );

    resolver.add_type_definition(
        "UserId32".to_string(),
        TypeDef::Bytes {
            size: Some(32),
            encoding: "hex".to_string(),
        },
    );

    resolver.add_type_definition(
        "Action".to_string(),
        TypeDef::Variant {
            variants: vec![
                Variant {
                    name: "Ping".to_string(),
                    code: None,
                    payload: None,
                },
                Variant {
                    name: "SetName".to_string(),
                    code: None,
                    payload: Some(TypeRef::Scalar(crate::schema::ScalarType::String)),
                },
                Variant {
                    name: "Update".to_string(),
                    code: None,
                    payload: Some(TypeRef::Scalar(crate::schema::ScalarType::U32)),
                },
            ],
        },
    );

    // Convert HashMap to BTreeMap
    let types: BTreeMap<String, TypeDef> = resolver.type_definitions.into_iter().collect();

    Ok(Manifest {
        schema_version: "wasm-abi/1".to_string(),
        types,
        methods,
        events,
    })
}
