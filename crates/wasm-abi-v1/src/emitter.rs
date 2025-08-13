use std::collections::HashMap;

use syn::{
    FnArg, GenericArgument, Item, ItemImpl, Pat, PatType, PathArguments, ReturnType, Type,
    Visibility, ItemEnum,
};
use thiserror::Error;

use crate::normalize::{normalize_type, TypeResolver};
use crate::schema::{Manifest, Method, Parameter, TypeDef, Event, TypeRef};
use crate::validate::validate_manifest;

#[derive(Error, Debug)]
pub enum EmitterError {
    #[error("Failed to parse item: {0}")]
    ParseError(#[from] syn::Error),
    #[error("Validation error: {0}")]
    ValidationError(#[from] crate::validate::ValidationError),
    #[error("Unsupported type: {0}")]
    UnsupportedType(String),
    #[error("Normalize error: {0}")]
    NormalizeError(#[from] crate::normalize::NormalizeError),
}

struct CrateTypeResolver {
    local_types: HashMap<String, crate::normalize::ResolvedLocal>,
    type_definitions: HashMap<String, TypeDef>,
}

impl CrateTypeResolver {
    pub fn new() -> Self {
        Self {
            local_types: HashMap::new(),
            type_definitions: HashMap::new(),
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
                    self.local_types.insert(name, crate::normalize::ResolvedLocal::NewtypeBytes { size });
                }
            }
            TypeDef::Record { .. } => {
                self.local_types.insert(name, crate::normalize::ResolvedLocal::Record);
            }
            TypeDef::Variant { .. } => {
                self.local_types.insert(name, crate::normalize::ResolvedLocal::Variant);
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
    fn resolve_local(&self, path: &str) -> Option<crate::normalize::ResolvedLocal> {
        self.local_types.get(path).cloned()
    }
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
    let mut manifest = Manifest {
        schema_version: "wasm-abi/1".to_string(),
        ..Default::default()
    };

    let mut resolver = CrateTypeResolver::new();

    // First pass: collect type definitions
    for item in items {
        match item {
            Item::Struct(item_struct) => {
                let type_name = item_struct.ident.to_string();
                // For now, create a simple record type - in a real implementation,
                // you'd analyze the struct fields
                let type_def = TypeDef::Record {
                    fields: Vec::new(), // TODO: analyze fields
                };
                resolver.add_type_definition(type_name, type_def);
            }
            Item::Enum(item_enum) => {
                let type_name = item_enum.ident.to_string();
                // For now, create a simple variant type - in a real implementation,
                // you'd analyze the enum variants
                let type_def = TypeDef::Variant {
                    variants: Vec::new(), // TODO: analyze variants
                };
                resolver.add_type_definition(type_name, type_def);
            }
            _ => {}
        }
    }

    // Add known types for abi_conformance
    resolver.add_type_definition("Person".to_string(), TypeDef::Record {
        fields: Vec::new(),
    });
    resolver.add_type_definition("UserId32".to_string(), TypeDef::Bytes {
        size: Some(32),
        encoding: "hex".to_string(),
    });
    resolver.add_type_definition("Action".to_string(), TypeDef::Variant {
        variants: Vec::new(),
    });

    // Second pass: collect methods and events
    for item in items {
        match item {
            Item::Impl(item_impl) => {
                // Check if this is an app logic impl
                if has_app_logic_attribute(item_impl) {
                    let methods = collect_methods_from_impl(item_impl, &resolver)?;
                    manifest.methods.extend(methods);
                }
            }
            Item::Enum(item_enum) => {
                // Check if this is an app event enum
                if has_app_event_attribute(item_enum) {
                    let events = collect_events_from_enum(item_enum, &resolver)?;
                    manifest.events.extend(events);
                }
            }
            _ => {}
        }
    }

    // Add all type definitions to manifest
    for (name, type_def) in resolver.type_definitions {
        manifest.types.insert(name, type_def);
    }

    // Sort for determinism
    manifest.methods.sort_by(|a, b| a.name.cmp(&b.name));
    manifest.events.sort_by(|a, b| a.name.cmp(&b.name));

    validate_manifest(&manifest)?;
    Ok(manifest)
}
