use crate::normalize::{normalize_type, TypeResolver};
use crate::schema::{Manifest, Method, Parameter, TypeDef};
use crate::validate::validate_manifest;
use std::collections::HashMap;
use syn::{
    FnArg, GenericArgument, Item, ItemImpl, Pat, PatType, PathArguments, ReturnType, Type, Visibility,
};
use thiserror::Error;

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
    #[allow(dead_code)]
    local_types: HashMap<String, TypeDef>,
    type_definitions: HashMap<String, TypeDef>,
}

impl CrateTypeResolver {
    pub fn new() -> Self {
        Self {
            local_types: HashMap::new(),
            type_definitions: HashMap::new(),
        }
    }
}

impl Default for CrateTypeResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeResolver for CrateTypeResolver {
    fn resolve_local(&self, _path: &str) -> Option<crate::normalize::ResolvedLocal> {
        None // Simplified implementation for now
    }
}

fn has_app_logic_attribute(item_impl: &ItemImpl) -> bool {
    item_impl.attrs.iter().any(|attr| {
        attr.path()
            .get_ident()
            .map(|ident| ident == "app")
            .unwrap_or(false)
    })
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
                let nullable = is_option_type(ty);
                params.push(Parameter {
                    name: pat_ident.ident.to_string(),
                    type_: type_ref,
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
            let nullable = is_option_type(ty);
            (Some(type_ref), if nullable { Some(true) } else { None })
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
    
    let resolver = CrateTypeResolver::new();
    
    for item in items {
        if let Item::Impl(item_impl) = item {
            // Check if this is an app logic impl
            if has_app_logic_attribute(item_impl) {
                let methods = collect_methods_from_impl(item_impl, &resolver)?;
                manifest.methods.extend(methods);
            }
        }
    }
    
    // Sort for determinism
    manifest.methods.sort_by(|a, b| a.name.cmp(&b.name));
    manifest.events.sort_by(|a, b| a.name.cmp(&b.name));
    
    validate_manifest(&manifest)?;
    Ok(manifest)
}
