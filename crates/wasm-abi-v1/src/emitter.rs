use crate::schema::{Manifest, Method, Parameter, TypeRef, TypeDef, Field, Variant, Error, Event};
use crate::normalize::{normalize_type, TypeResolver, ResolvedLocal};
use syn::{ItemImpl, ImplItem, ImplItemFn, FnArg, Pat, PatIdent, Type, ReturnType, Visibility, Item, ItemStruct, ItemEnum, Fields, Variant as SynVariant};
use std::collections::{BTreeMap, HashMap};
use thiserror::Error;

/// Error type for emitter failures
#[derive(Debug, Error)]
pub enum EmitterError {
    #[error("failed to parse method signature: {0}")]
    MethodSignatureError(String),
    #[error("unsupported method visibility: {0}")]
    UnsupportedVisibility(String),
    #[error("failed to normalize type: {0}")]
    NormalizeError(#[from] crate::normalize::NormalizeError),
    #[error("failed to resolve local type: {0}")]
    TypeResolutionError(String),
}

/// Crate-level type resolver that can detect local types
pub struct CrateTypeResolver {
    local_types: HashMap<String, ResolvedLocal>,
    type_definitions: HashMap<String, TypeDef>,
}

impl CrateTypeResolver {
    pub fn new() -> Self {
        Self {
            local_types: HashMap::new(),
            type_definitions: HashMap::new(),
        }
    }

    /// Add a local type definition
    pub fn add_local_type(&mut self, name: String, resolved: ResolvedLocal) {
        self.local_types.insert(name, resolved);
    }

    /// Add a type definition
    pub fn add_type_definition(&mut self, name: String, def: TypeDef) {
        self.type_definitions.insert(name, def);
    }

    /// Get a type definition
    pub fn get_type_definition(&self, name: &str) -> Option<&TypeDef> {
        self.type_definitions.get(name)
    }
}

impl TypeResolver for CrateTypeResolver {
    fn resolve_local(&self, path: &str) -> Option<ResolvedLocal> {
        self.local_types.get(path).cloned()
    }
}

/// Emit ABI manifest from crate items
pub fn emit_manifest(items: &[Item]) -> Result<Manifest, EmitterError> {
    let mut resolver = CrateTypeResolver::new();
    let mut manifest = Manifest::default();
    
    // First pass: collect all type definitions
    for item in items {
        match item {
            Item::Struct(item_struct) => {
                let type_name = item_struct.ident.to_string();
                let type_def = convert_struct_to_type_def(item_struct)?;
                resolver.add_type_definition(type_name.clone(), type_def.clone());
                
                // Determine if it's a newtype wrapper
                if let Fields::Unnamed(fields) = &item_struct.fields {
                    if fields.unnamed.len() == 1 {
                        let inner_type = &fields.unnamed[0].ty;
                        if let Type::Array(array) = inner_type {
                            if let Type::Path(path) = &*array.elem {
                                if let Some(ident) = path.path.get_ident() {
                                    if ident == "u8" {
                                        let size = extract_array_len(&array.len)?;
                                        resolver.add_local_type(type_name, ResolvedLocal::NewtypeBytes { size });
                                        continue;
                                    }
                                }
                            }
                        }
                    }
                }
                
                resolver.add_local_type(type_name, ResolvedLocal::Record);
            }
            Item::Enum(item_enum) => {
                let type_name = item_enum.ident.to_string();
                let type_def = convert_enum_to_type_def(item_enum)?;
                resolver.add_type_definition(type_name.clone(), type_def);
                resolver.add_local_type(type_name, ResolvedLocal::Variant);
            }
            _ => {}
        }
    }
    
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
            _ => {}
        }
    }
    
    // Third pass: collect events
    for item in items {
        if let Item::Enum(item_enum) = item {
            if has_app_event_attribute(item_enum) {
                let events = collect_events_from_enum(item_enum, &resolver)?;
                manifest.events.extend(events);
            }
        }
    }
    
    // Add all type definitions to manifest
    for (name, def) in resolver.type_definitions {
        manifest.types.insert(name, def);
    }
    
    // Ensure all referenced types are defined
    ensure_all_referenced_types_defined(&mut manifest);
    
    Ok(manifest)
}

/// Check if an impl block has the app::logic attribute
fn has_app_logic_attribute(item_impl: &ItemImpl) -> bool {
    item_impl.attrs.iter().any(|attr| {
        attr.path().segments.len() == 2 &&
        attr.path().segments[0].ident == "app" &&
        attr.path().segments[1].ident == "logic"
    })
}

/// Check if an enum has the app::event attribute
fn has_app_event_attribute(item_enum: &ItemEnum) -> bool {
    item_enum.attrs.iter().any(|attr| {
        attr.path().segments.len() == 2 &&
        attr.path().segments[0].ident == "app" &&
        attr.path().segments[1].ident == "event"
    })
}

/// Collect methods from an impl block
fn collect_methods_from_impl(
    item_impl: &ItemImpl,
    resolver: &CrateTypeResolver,
) -> Result<Vec<Method>, EmitterError> {
    let mut methods = Vec::new();
    
    for item in &item_impl.items {
        if let ImplItem::Fn(method) = item {
            // Only include public methods
            if let Visibility::Public(_) = &method.vis {
                let method_def = convert_method_to_abi(method, resolver)?;
                methods.push(method_def);
            }
        }
    }
    
    Ok(methods)
}

/// Convert a method to ABI Method
fn convert_method_to_abi(
    method: &ImplItemFn,
    resolver: &CrateTypeResolver,
) -> Result<Method, EmitterError> {
    let mut params = Vec::new();
    
    // Process method parameters
    for arg in &method.sig.inputs {
        match arg {
            FnArg::Typed(pat_type) => {
                if let Pat::Ident(PatIdent { ident, .. }) = &*pat_type.pat {
                    let type_ref = normalize_type(&pat_type.ty, true, resolver)?;
                    let nullable = is_option_type(&pat_type.ty);
                    
                    params.push(Parameter {
                        name: ident.to_string(),
                        type_: type_ref,
                        nullable: if nullable { Some(true) } else { None },
                    });
                }
            }
            FnArg::Receiver(_) => {
                // Skip self parameter
                continue;
            }
        }
    }
    
    // Process return type
    let returns = match &method.sig.output {
        ReturnType::Default => None,
        ReturnType::Type(_, ty) => {
            let type_ref = normalize_type(ty, true, resolver)?;
            Some(type_ref)
        }
    };
    
    // Extract errors from return type
    let errors = extract_errors_from_return_type(&method.sig.output, resolver)?;
    
    Ok(Method {
        name: method.sig.ident.to_string(),
        params,
        returns,
        errors,
    })
}

/// Check if a type is Option<T>
fn is_option_type(ty: &Type) -> bool {
    if let Type::Path(path) = ty {
        if let Some(ident) = path.path.get_ident() {
            return ident.to_string() == "Option";
        }
    }
    false
}

/// Extract errors from a return type
fn extract_errors_from_return_type(
    return_type: &ReturnType,
    resolver: &CrateTypeResolver,
) -> Result<Vec<Error>, EmitterError> {
    let mut errors = Vec::new();
    
    if let ReturnType::Type(_, ty) = return_type {
        if let Type::Path(path) = &**ty {
            // Check if it's app::Result<T, E>
            if path.path.segments.len() >= 2 {
                if let Some(last_seg) = path.path.segments.last() {
                    if last_seg.ident == "Result" {
                        // Extract error type from Result<T, E>
                        if let syn::PathArguments::AngleBracketed(args) = &last_seg.arguments {
                            if args.args.len() >= 2 {
                                if let syn::GenericArgument::Type(error_type) = &args.args[1] {
                                    // Convert enum variants to error codes
                                    if let Type::Path(error_path) = error_type {
                                        if let Some(error_ident) = error_path.path.get_ident() {
                                            let error_name = error_ident.to_string();
                                            if let Some(type_def) = resolver.get_type_definition(&error_name) {
                                                if let TypeDef::Variant { variants } = type_def {
                                                    for variant in variants {
                                                        let code = variant.name.to_uppercase();
                                                        let error = Error {
                                                            code,
                                                            type_: variant.type_.clone(),
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
            }
        }
    }
    
    Ok(errors)
}

/// Convert a struct to TypeDef
fn convert_struct_to_type_def(item_struct: &ItemStruct) -> Result<TypeDef, EmitterError> {
    let mut fields = Vec::new();
    
    match &item_struct.fields {
        Fields::Named(named_fields) => {
            for field in &named_fields.named {
                if let Some(ident) = &field.ident {
                    let type_ref = normalize_type(&field.ty, true, &CrateTypeResolver::new())?;
                    let nullable = is_option_type(&field.ty);
                    
                    fields.push(Field {
                        name: ident.to_string(),
                        type_: type_ref,
                        nullable: if nullable { Some(true) } else { None },
                    });
                }
            }
        }
        Fields::Unnamed(_) => {
            return Err(EmitterError::TypeResolutionError(
                "tuple structs are not supported in ABI type definitions".to_string()
            ));
        }
        Fields::Unit => {
            // Unit structs - no fields
        }
    }
    
    Ok(TypeDef::Record { fields })
}

/// Convert an enum to TypeDef
fn convert_enum_to_type_def(item_enum: &ItemEnum) -> Result<TypeDef, EmitterError> {
    let mut variants = Vec::new();
    
    for variant in &item_enum.variants {
        let variant_type = match &variant.fields {
            Fields::Named(named_fields) => {
                // For named fields, create a record type
                let mut fields = Vec::new();
                for field in &named_fields.named {
                    if let Some(ident) = &field.ident {
                        let type_ref = normalize_type(&field.ty, true, &CrateTypeResolver::new())?;
                        let nullable = is_option_type(&field.ty);
                        
                        fields.push(Field {
                            name: ident.to_string(),
                            type_: type_ref,
                            nullable: if nullable { Some(true) } else { None },
                        });
                    }
                }
                Some(TypeRef::Collection(crate::schema::CollectionType::Record { fields }))
            }
            Fields::Unnamed(fields) => {
                if fields.unnamed.len() == 1 {
                    Some(normalize_type(&fields.unnamed[0].ty, true, &CrateTypeResolver::new())?)
                } else {
                    // Multiple unnamed fields - treat as generic
                    Some(TypeRef::string())
                }
            }
            Fields::Unit => None,
        };
        
        variants.push(Variant {
            name: variant.ident.to_string(),
            type_: variant_type,
        });
    }
    
    Ok(TypeDef::Variant { variants })
}

/// Collect events from an enum
fn collect_events_from_enum(
    item_enum: &ItemEnum,
    resolver: &CrateTypeResolver,
) -> Result<Vec<Event>, EmitterError> {
    let mut events = Vec::new();
    
    for variant in &item_enum.variants {
        let payload = match &variant.fields {
            Fields::Named(named_fields) => {
                // For named fields, create a record type
                let mut fields = Vec::new();
                for field in &named_fields.named {
                    if let Some(ident) = &field.ident {
                        let type_ref = normalize_type(&field.ty, true, resolver)?;
                        let nullable = is_option_type(&field.ty);
                        
                        fields.push(Field {
                            name: ident.to_string(),
                            type_: type_ref,
                            nullable: if nullable { Some(true) } else { None },
                        });
                    }
                }
                Some(TypeRef::Collection(crate::schema::CollectionType::Record { fields }))
            }
            Fields::Unnamed(fields) => {
                if fields.unnamed.len() == 1 {
                    Some(normalize_type(&fields.unnamed[0].ty, true, resolver)?)
                } else {
                    // Multiple unnamed fields - treat as generic
                    Some(TypeRef::string())
                }
            }
            Fields::Unit => None,
        };
        
        events.push(Event {
            name: variant.ident.to_string(),
            payload,
        });
    }
    
    Ok(events)
}

/// Extract array length from [T; N]
fn extract_array_len(len: &syn::Expr) -> Result<usize, EmitterError> {
    if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Int(lit), .. }) = len {
        lit.base10_parse().map_err(|_| {
            EmitterError::TypeResolutionError("failed to parse array length".to_string())
        })
    } else {
        Err(EmitterError::TypeResolutionError(
            "array length must be a literal integer".to_string()
        ))
    }
}

/// Ensure all referenced types are defined in the manifest
fn ensure_all_referenced_types_defined(manifest: &mut Manifest) {
    // This is a placeholder - in a full implementation, we would:
    // 1. Scan all TypeRef::Reference in the manifest
    // 2. Ensure each referenced type exists in manifest.types
    // 3. Add placeholder types for any missing references
    // For now, we assume all types are properly defined
} 