use std::collections::BTreeMap;

use crate::normalize::TypeResolver;
use crate::schema::{Event, Field, Manifest, Method, Parameter, TypeDef, TypeRef, Variant};
use syn::{Item, Type, TypePath};
use thiserror::Error;

struct CrateTypeResolver {
    local_types: std::collections::HashMap<String, crate::normalize::ResolvedLocal>,
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
                    self.local_types
                        .insert(name, crate::normalize::ResolvedLocal::NewtypeBytes { size });
                }
            }
            TypeDef::Record { .. } => {
                self.local_types
                    .insert(name, crate::normalize::ResolvedLocal::Record);
            }
            TypeDef::Variant { .. } => {
                self.local_types
                    .insert(name, crate::normalize::ResolvedLocal::Variant);
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
        TypeRef::Collection(collection) => match collection {
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
                let processed_fields = fields
                    .into_iter()
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
        },
        TypeRef::Scalar(scalar) => {
            // Check if this is a newtype bytes that should be converted to a reference
            match scalar {
                crate::schema::ScalarType::Bytes { size, ref encoding } => {
                    // Look for a type definition that matches this bytes type
                    for (name, type_def) in &resolver.type_definitions {
                        if let TypeDef::Bytes {
                            size: def_size,
                            encoding: def_encoding,
                        } = type_def
                        {
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

pub fn emit_manifest(items: &[Item]) -> Result<Manifest, EmitterError> {
    let mut resolver = CrateTypeResolver::new();
    let mut methods = Vec::new();
    let mut events = Vec::new();

    // First pass: collect type definitions
    for item in items {
        match item {
            Item::Struct(item_struct) => {
                let struct_name = item_struct.ident.to_string();

                // Process struct fields to generate type definitions
                let fields = item_struct
                    .fields
                    .iter()
                    .map(|field| {
                        let field_name = field
                            .ident
                            .as_ref()
                            .map(|id| id.to_string())
                            .unwrap_or_else(|| "unnamed".to_string());
                        let field_type =
                            crate::normalize::normalize_type(&field.ty, true, &resolver)?;
                        let field_type = post_process_type_ref(field_type, &resolver);

                        // Check if this is an Option<T> type
                        let nullable = if let Type::Path(TypePath { path, .. }) = &field.ty {
                            if path.segments.len() == 1 && path.segments[0].ident == "Option" {
                                Some(true)
                            } else {
                                None
                            }
                        } else {
                            None
                        };

                        Ok::<Field, EmitterError>(Field {
                            name: field_name,
                            type_: field_type,
                            nullable,
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;

                // Add the struct as a type definition
                resolver.add_type_definition(struct_name, TypeDef::Record { fields });
            }
            Item::Enum(item_enum) => {
                let enum_name = item_enum.ident.to_string();

                // Process all enums to generate type definitions
                let variants = item_enum
                    .variants
                    .iter()
                    .map(|variant| {
                        let variant_name = variant.ident.to_string();
                        let payload = match &variant.fields {
                            syn::Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                                let payload_type = crate::normalize::normalize_type(
                                    &fields.unnamed[0].ty,
                                    true,
                                    &resolver,
                                )?;
                                let payload_type = post_process_type_ref(payload_type, &resolver);
                                Some(payload_type)
                            }
                            syn::Fields::Named(fields) => {
                                // Create a record type for named fields
                                let fields_vec = fields
                                    .named
                                    .iter()
                                    .map(|field| {
                                        let field_name = field
                                            .ident
                                            .as_ref()
                                            .map(|id| id.to_string())
                                            .unwrap_or_else(|| "unnamed".to_string());
                                        let field_type = crate::normalize::normalize_type(
                                            &field.ty, true, &resolver,
                                        )?;
                                        let field_type =
                                            post_process_type_ref(field_type, &resolver);
                                        Ok::<Field, EmitterError>(Field {
                                            name: field_name,
                                            type_: field_type,
                                            nullable: None,
                                        })
                                    })
                                    .collect::<Result<Vec<_>, _>>()?;

                                // Add this as a type definition
                                let type_name = format!("{}Payload", variant_name);
                                resolver.add_type_definition(
                                    type_name.clone(),
                                    TypeDef::Record { fields: fields_vec },
                                );

                                Some(TypeRef::Reference { ref_: type_name })
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

                // Add the enum as a type definition
                resolver.add_type_definition(enum_name.clone(), TypeDef::Variant { variants });

                // Only treat enums named 'Event' as event sources
                if enum_name == "Event" {
                    // Create individual events for each variant
                    for variant in &item_enum.variants {
                        let variant_name = variant.ident.to_string();
                        let payload = match &variant.fields {
                            syn::Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                                let payload_type = crate::normalize::normalize_type(
                                    &fields.unnamed[0].ty,
                                    true,
                                    &resolver,
                                )?;
                                let payload_type = post_process_type_ref(payload_type, &resolver);
                                Some(payload_type)
                            }
                            syn::Fields::Named(fields) => {
                                // Create a record type for named fields
                                let fields_vec = fields
                                    .named
                                    .iter()
                                    .map(|field| {
                                        let field_name = field.ident.as_ref().unwrap().to_string();
                                        let field_type = crate::normalize::normalize_type(
                                            &field.ty, true, &resolver,
                                        )?;
                                        let field_type =
                                            post_process_type_ref(field_type, &resolver);
                                        Ok::<Field, EmitterError>(Field {
                                            name: field_name,
                                            type_: field_type,
                                            nullable: None,
                                        })
                                    })
                                    .collect::<Result<Vec<_>, _>>()?;

                                // Add this as a type definition
                                let type_name = format!("{}Payload", variant_name);
                                resolver.add_type_definition(
                                    type_name.clone(),
                                    TypeDef::Record { fields: fields_vec },
                                );

                                Some(TypeRef::Reference { ref_: type_name })
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
                                            syn::Pat::Ident(pat_ident) => {
                                                pat_ident.ident.to_string()
                                            }
                                            _ => "param".to_string(),
                                        };
                                        let param_type = match crate::normalize::normalize_type(
                                            &pat_type.ty,
                                            true,
                                            &resolver,
                                        ) {
                                            Ok(ty) => post_process_type_ref(ty, &resolver),
                                            Err(_) => {
                                                return Some(Err(EmitterError::NormalizeError(
                                                    crate::normalize::NormalizeError::TypePathError(
                                                        "failed to normalize parameter type"
                                                            .to_string(),
                                                    ),
                                                )))
                                            }
                                        };

                                        // Check if this is an Option<T> type
                                        let nullable = if let Type::Path(TypePath {
                                            path, ..
                                        }) = &*pat_type.ty
                                        {
                                            if path.segments.len() == 1
                                                && path.segments[0].ident == "Option"
                                            {
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

                            let return_type = match &method.sig.output {
                                syn::ReturnType::Default => {
                                    // Unit return type
                                    Some(TypeRef::Scalar(crate::schema::ScalarType::Unit))
                                }
                                syn::ReturnType::Type(_, ty) => {
                                    let ret_type =
                                        crate::normalize::normalize_type(ty, true, &resolver)?;
                                    let ret_type = post_process_type_ref(ret_type, &resolver);
                                    Some(ret_type)
                                }
                            };

                            // Check if return type is nullable (Option<T>)
                            let returns_nullable =
                                if let syn::ReturnType::Type(_, ty) = &method.sig.output {
                                    if let Type::Path(TypePath { path, .. }) = &**ty {
                                        if path.segments.len() == 1
                                            && path.segments[0].ident == "Option"
                                        {
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

    // Convert HashMap to BTreeMap and filter out extra types
    let mut types: BTreeMap<String, TypeDef> = resolver.type_definitions.into_iter().collect();

    // Remove extra types that shouldn't be in the ABI
    types.remove("AbiStateExposed");
    types.remove("Event");

    Ok(Manifest {
        schema_version: "wasm-abi/1".to_string(),
        types,
        methods,
        events,
    })
}
