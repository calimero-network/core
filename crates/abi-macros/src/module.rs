// Copyright 2024 Calimero Limited
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, AttributeArgs, ItemMod, Item, Visibility, parse::Parse, parse::ParseStream, Ident, Token, LitStr, Type, PathArguments};
use std::fs;
use std::path::Path;
use sha2::{Digest, Sha256};

#[cfg(feature = "abi-export")]
use abi_core::{Abi, AbiMetadata, AbiFunction, AbiEvent, AbiParameter, AbiTypeRef, TypeDef, FieldDef, VariantDef, VariantKind, MapMode};
#[cfg(feature = "abi-export")]
use abi_core::schema::{FunctionKind as CoreFunctionKind, ParameterDirection, ErrorAbi};

use crate::types::AbiTypeRef as MacroAbiTypeRef;

/// Module attributes
#[derive(Debug)]
struct ModuleAttrs {
    name: String,
    version: String,
}

impl Parse for ModuleAttrs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut name = None;
        let mut version = None;
        
        while !input.is_empty() {
            let ident: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            let lit: LitStr = input.parse()?;
            
            match ident.to_string().as_str() {
                "name" => name = Some(lit.value()),
                "version" => version = Some(lit.value()),
                _ => return Err(syn::Error::new_spanned(&ident, "unknown attribute")),
            }
            
            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }
        
        let name = name.ok_or_else(|| syn::Error::new(input.span(), "name is required"))?;
        let version = version.ok_or_else(|| syn::Error::new(input.span(), "version is required"))?;
        
        Ok(ModuleAttrs { name, version })
    }
}

/// Function information collected from module
#[derive(Debug)]
struct FunctionInfo {
    name: String,
    kind: FunctionKind,
    parameters: Vec<ParameterInfo>,
    returns: Option<MacroAbiTypeRef>,
    errors: Vec<ErrorInfo>,
}

#[derive(Debug)]
enum FunctionKind {
    Query,
    Command,
}

#[derive(Debug)]
struct ParameterInfo {
    name: String,
    ty: MacroAbiTypeRef,
}

#[derive(Debug)]
struct ErrorInfo {
    name: String,
    code: String,
    ty: Option<MacroAbiTypeRef>,
}

/// Event information collected from module
#[derive(Debug)]
struct EventInfo {
    name: String,
    payload_type: Option<MacroAbiTypeRef>,
}

/// Type information collected from module
#[derive(Debug)]
#[cfg(feature = "abi-export")]
struct TypeInfo {
    name: String,
    ty: TypeDef,
}

pub fn module_impl(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = parse_macro_input!(attr as AttributeArgs);
    let item = parse_macro_input!(item as ItemMod);
    
    // Parse attributes
    let attrs = match parse_module_attrs(&attr) {
        Ok(attrs) => attrs,
        Err(e) => return e.to_compile_error().into(),
    };
    
    // Collect functions, events, and types from module
    let mut functions = Vec::new();
    let mut events = Vec::new();
    #[cfg(feature = "abi-export")]
    let mut types: Vec<TypeInfo> = Vec::new();
    #[cfg(not(feature = "abi-export"))]
    let mut types: Vec<()> = Vec::new();
    
    if let Some((_, ref content)) = item.content {
        for item in content {
            match item {
                Item::Fn(func) => {
                    if let Some(func_info) = collect_function(&func) {
                        functions.push(func_info);
                    }
                }
                Item::Struct(struct_item) => {
                    if let Some(event_info) = collect_event(&struct_item) {
                        events.push(event_info);
                    }
                    // Also collect types for registry
                    #[cfg(feature = "abi-export")]
                    if let Some(type_info) = collect_type(&struct_item) {
                        types.push(type_info);
                    }
                }
                Item::Enum(enum_item) => {
                    // Collect enum types for registry
                    #[cfg(feature = "abi-export")]
                    if let Some(type_info) = collect_enum_type(&enum_item) {
                        types.push(type_info);
                    }
                }
                _ => {}
            }
        }
    }
    
    // Generate ABI JSON and write ABI file
    #[cfg(feature = "abi-export")]
    {
        let abi_json = generate_abi_json(&attrs, &functions, &events, &types);
        if let Ok(json_bytes) = serde_json::to_vec_pretty(&abi_json) {
            let _ = abi_core::build::emit_if_enabled(&attrs.name, &json_bytes);
        }
    }
    
    // Generate the module with ABI_PATH constant
    let abi_path_const = generate_abi_path_constant(&attrs.name);
    
    let expanded = quote! {
        #item
        
        #abi_path_const
    };
    
    expanded.into()
}

fn parse_module_attrs(attr: &AttributeArgs) -> syn::Result<ModuleAttrs> {
    let mut name = None;
    let mut version = None;
    
    for arg in attr {
        match arg {
            syn::NestedMeta::Meta(syn::Meta::NameValue(name_value)) => {
                if name_value.path.is_ident("name") {
                    if let syn::Lit::Str(lit) = &name_value.lit {
                        name = Some(lit.value());
                    }
                } else if name_value.path.is_ident("version") {
                    if let syn::Lit::Str(lit) = &name_value.lit {
                        version = Some(lit.value());
                    }
                } else {
                    return Err(syn::Error::new_spanned(name_value, 
                        format!("unknown attribute '{}'. Expected 'name' or 'version'", 
                            name_value.path.get_ident().map(|i| i.to_string()).unwrap_or_default())));
                }
            }
            _ => {
                return Err(syn::Error::new_spanned(arg, "expected name-value attribute"));
            }
        }
    }
    
    let name = name.ok_or_else(|| syn::Error::new(proc_macro2::Span::call_site(), 
        "missing required 'name' attribute. Use #[abi::module(name = \"module_name\", version = \"0.1.0\")]"))?;
    let version = version.ok_or_else(|| syn::Error::new(proc_macro2::Span::call_site(), 
        "missing required 'version' attribute. Use #[abi::module(name = \"module_name\", version = \"0.1.0\")]"))?;
    
    Ok(ModuleAttrs { name, version })
}

fn collect_function(func: &syn::ItemFn) -> Option<FunctionInfo> {
    // Check if function is public
    if !matches!(func.vis, Visibility::Public(_)) {
        return None;
    }
    
    // Check for query/command attributes
    let mut kind = None;
    for attr in &func.attrs {
        if attr.path.is_ident("query") || attr.path.segments.last().map(|s| s.ident == "query").unwrap_or(false) {
            kind = Some(FunctionKind::Query);
            break;
        } else if attr.path.is_ident("command") || attr.path.segments.last().map(|s| s.ident == "command").unwrap_or(false) {
            kind = Some(FunctionKind::Command);
            break;
        }
    }
    
    let kind = kind?;
    
    // Parse function signature
    let mut parameters = Vec::new();
    for param in &func.sig.inputs {
        match param {
            syn::FnArg::Typed(pat_type) => {
                if let syn::Pat::Ident(pat_ident) = &*pat_type.pat {
                    let param_name = pat_ident.ident.to_string();
                    let param_type = MacroAbiTypeRef::from_rust_type(&pat_type.ty)
                        .unwrap_or_else(|_| MacroAbiTypeRef::Ref("unknown".to_string()));
                    
                    parameters.push(ParameterInfo {
                        name: param_name,
                        ty: param_type,
                    });
                }
            }
            _ => {}
        }
    }
    
    // Parse return type
    let returns = match &func.sig.output {
        syn::ReturnType::Default => None,
        syn::ReturnType::Type(_, ty) => {
            MacroAbiTypeRef::from_rust_type(&**ty).ok()
        }
    };
    
    // Generate errors based on return type
    let errors = generate_errors_for_function(&func.sig.output);
    
    Some(FunctionInfo {
        name: func.sig.ident.to_string(),
        kind,
        parameters,
        returns,
        errors,
    })
}

fn collect_event(struct_item: &syn::ItemStruct) -> Option<EventInfo> {
    // Check if struct is public
    if !matches!(struct_item.vis, Visibility::Public(_)) {
        return None;
    }
    
    // Check for event attribute
    for attr in &struct_item.attrs {
        if attr.path.is_ident("event") || attr.path.segments.last().map(|s| s.ident == "event").unwrap_or(false) {
            return Some(EventInfo {
                name: struct_item.ident.to_string(),
                payload_type: None, // For now, events don't have payload types
            });
        }
    }
    
    None
}

#[cfg(feature = "abi-export")]
fn collect_type(struct_item: &syn::ItemStruct) -> Option<TypeInfo> {
    // Check if struct is public
    if !matches!(struct_item.vis, Visibility::Public(_)) {
        return None;
    }
    
    // Check for AbiType derive
    for attr in &struct_item.attrs {
        if attr.path.is_ident("derive") {
            if let Ok(syn::Meta::List(list)) = attr.parse_meta() {
                for nested in list.nested {
                    if let syn::NestedMeta::Meta(syn::Meta::Path(path)) = nested {
                        if path.is_ident("AbiType") {
                            // This struct has AbiType derive, collect it
                            let name = struct_item.ident.to_string();
                            let fields = struct_item.fields.iter().map(|field| {
                                let field_name = field.ident.as_ref().unwrap().to_string();
                                let field_type = MacroAbiTypeRef::from_rust_type(&field.ty)
                                    .unwrap_or_else(|_| MacroAbiTypeRef::Ref("unknown".to_string()));
                                FieldDef {
                                    name: field_name,
                                    ty: convert_macro_type_to_core_type(&field_type),
                                }
                            }).collect();
                            
                            let newtype = matches!(struct_item.fields, syn::Fields::Unnamed(_) if struct_item.fields.len() == 1);
                            
                            return Some(TypeInfo {
                                name,
                                ty: TypeDef::Struct { fields, newtype },
                            });
                        }
                    }
                }
            }
        }
    }
    
    None
}

#[cfg(feature = "abi-export")]
fn collect_enum_type(enum_item: &syn::ItemEnum) -> Option<TypeInfo> {
    // Check if enum is public
    if !matches!(enum_item.vis, Visibility::Public(_)) {
        return None;
    }
    
    // Check for AbiType derive
    for attr in &enum_item.attrs {
        if attr.path.is_ident("derive") {
            if let Ok(syn::Meta::List(list)) = attr.parse_meta() {
                for nested in list.nested {
                    if let syn::NestedMeta::Meta(syn::Meta::Path(path)) = nested {
                        if path.is_ident("AbiType") {
                            // This enum has AbiType derive, collect it
                            let name = enum_item.ident.to_string();
                            let variants = enum_item.variants.iter().map(|variant| {
                                let variant_name = variant.ident.to_string();
                                let variant_kind = match &variant.fields {
                                    syn::Fields::Unit => VariantKind::Unit,
                                    syn::Fields::Unnamed(fields) => {
                                        let items = fields.unnamed.iter().map(|field| {
                                            MacroAbiTypeRef::from_rust_type(&field.ty)
                                                .map(|t| convert_macro_type_to_core_type(&t))
                                                .unwrap_or_else(|_| AbiTypeRef::inline_primitive("unknown".to_string()))
                                        }).collect();
                                        VariantKind::Tuple { items }
                                    }
                                    syn::Fields::Named(fields) => {
                                        let fields = fields.named.iter().map(|field| {
                                            let field_name = field.ident.as_ref().unwrap().to_string();
                                            let field_type = MacroAbiTypeRef::from_rust_type(&field.ty)
                                                .map(|t| convert_macro_type_to_core_type(&t))
                                                .unwrap_or_else(|_| AbiTypeRef::inline_primitive("unknown".to_string()));
                                            FieldDef {
                                                name: field_name,
                                                ty: field_type,
                                            }
                                        }).collect();
                                        VariantKind::Struct { fields }
                                    }
                                };
                                
                                VariantDef {
                                    name: variant_name,
                                    kind: variant_kind,
                                }
                            }).collect();
                            
                            return Some(TypeInfo {
                                name,
                                ty: TypeDef::Enum { variants },
                            });
                        }
                    }
                }
            }
        }
    }
    
    None
}

#[cfg(feature = "abi-export")]
fn convert_macro_type_to_core_type(macro_type: &MacroAbiTypeRef) -> AbiTypeRef {
    match macro_type {
        MacroAbiTypeRef::Bool => AbiTypeRef::inline_primitive("bool".to_string()),
        MacroAbiTypeRef::U8 => AbiTypeRef::inline_primitive("u8".to_string()),
        MacroAbiTypeRef::U16 => AbiTypeRef::inline_primitive("u16".to_string()),
        MacroAbiTypeRef::U32 => AbiTypeRef::inline_primitive("u32".to_string()),
        MacroAbiTypeRef::U64 => AbiTypeRef::inline_primitive("u64".to_string()),
        MacroAbiTypeRef::I8 => AbiTypeRef::inline_primitive("i8".to_string()),
        MacroAbiTypeRef::I16 => AbiTypeRef::inline_primitive("i16".to_string()),
        MacroAbiTypeRef::I32 => AbiTypeRef::inline_primitive("i32".to_string()),
        MacroAbiTypeRef::I64 => AbiTypeRef::inline_primitive("i64".to_string()),
        MacroAbiTypeRef::U128 => AbiTypeRef::inline_primitive("u128".to_string()),
        MacroAbiTypeRef::I128 => AbiTypeRef::inline_primitive("i128".to_string()),
        MacroAbiTypeRef::String => AbiTypeRef::inline_primitive("string".to_string()),
        MacroAbiTypeRef::Bytes => AbiTypeRef::inline_primitive("bytes".to_string()),
        MacroAbiTypeRef::Option(inner) => {
            let inner_core = convert_macro_type_to_core_type(inner);
            AbiTypeRef::inline_composite(
                "option".to_string(),
                Some(Box::new(inner_core)),
                None, None, None, None, None, None, None
            )
        }
        MacroAbiTypeRef::Vec(inner) => {
            let inner_core = convert_macro_type_to_core_type(inner);
            AbiTypeRef::inline_composite(
                "vec".to_string(),
                Some(Box::new(inner_core)),
                None, None, None, None, None, None, None
            )
        }
        MacroAbiTypeRef::Tuple(items) => {
            let items_core: Vec<AbiTypeRef> = items.iter().map(convert_macro_type_to_core_type).collect();
            AbiTypeRef::inline_composite(
                "tuple".to_string(),
                None,
                Some(items_core),
                None, None, None, None, None, None
            )
        }
        MacroAbiTypeRef::Array(item, len) => {
            let item_core = convert_macro_type_to_core_type(item);
            AbiTypeRef::inline_composite(
                "array".to_string(),
                Some(Box::new(item_core)),
                None,
                Some(*len),
                None, None, None, None, None
            )
        }
        MacroAbiTypeRef::Map(key, value, mode) => {
            let key_core = convert_macro_type_to_core_type(key);
            let value_core = convert_macro_type_to_core_type(value);
            let mode_core = match mode {
                crate::types::MapMode::Object => MapMode::Object,
                crate::types::MapMode::Entries => MapMode::Entries,
            };
            AbiTypeRef::inline_composite(
                "map".to_string(),
                None,
                None, None,
                Some(Box::new(key_core)),
                Some(mode_core),
                None, None, None
            )
        }
        MacroAbiTypeRef::Ref(name) => AbiTypeRef::ref_(name.clone()),
    }
}

fn generate_errors_for_function(return_type: &syn::ReturnType) -> Vec<ErrorInfo> {
    // For now, generate generic errors
    // In a full implementation, this would analyze the actual error types
    vec![
        ErrorInfo {
            name: "InvalidInput".to_string(),
            code: "INVALID_INPUT".to_string(),
            ty: Some(MacroAbiTypeRef::String),
        },
        ErrorInfo {
            name: "NotFound".to_string(),
            code: "NOT_FOUND".to_string(),
            ty: None,
        },
    ]
}

#[cfg(feature = "abi-export")]
fn generate_abi_json(attrs: &ModuleAttrs, functions: &[FunctionInfo], events: &[EventInfo], types: &[TypeInfo]) -> serde_json::Value {
    // Generate source hash
    let mut hasher = Sha256::new();
    hasher.update(format!("{}{}", attrs.name, attrs.version).as_bytes());
    let source_hash = hex::encode(hasher.finalize());
    
    // Create ABI using abi-core types
    let mut abi = Abi::new(
        attrs.name.clone(),
        attrs.version.clone(),
        "1.85.0".to_string(),
        source_hash,
    );
    
    // Add types to registry if any
    for type_info in types {
        abi.add_type(type_info.name.clone(), type_info.ty.clone());
    }
    
    // Add functions
    for func in functions {
        let kind = match func.kind {
            FunctionKind::Query => CoreFunctionKind::Query,
            FunctionKind::Command => CoreFunctionKind::Command,
        };
        
        let parameters = func.parameters.iter().map(|param| {
            AbiParameter {
                name: param.name.clone(),
                ty: convert_macro_type_to_core_type(&param.ty),
                direction: ParameterDirection::Input,
            }
        }).collect();
        
        let returns = func.returns.as_ref().map(|ret_ty| {
            convert_macro_type_to_core_type(ret_ty)
        });
        
        let errors = func.errors.iter().map(|error| {
            ErrorAbi {
                name: error.name.clone(),
                code: error.code.clone(),
                ty: error.ty.as_ref().map(|ty| convert_macro_type_to_core_type(ty)),
            }
        }).collect();
        
        let abi_function = AbiFunction {
            name: func.name.clone(),
            kind,
            parameters,
            returns,
            errors,
        };
        
        abi.add_function(abi_function);
    }
    
    // Add events
    for event in events {
        let abi_event = AbiEvent {
            name: event.name.clone(),
            payload_type: event.payload_type.as_ref().map(|ty| convert_macro_type_to_core_type(ty)),
        };
        
        abi.add_event(abi_event);
    }
    
    // Convert to JSON
    serde_json::to_value(abi).unwrap()
}

fn write_abi_file(module_name: &str, abi_json: &serde_json::Value) -> std::io::Result<()> {
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    let abi_dir = Path::new(&out_dir).join("calimero").join("abi");
    fs::create_dir_all(&abi_dir)?;
    
    let abi_file = abi_dir.join(format!("{}.json", module_name));
    let json_string = serde_json::to_string_pretty(abi_json).unwrap();
    fs::write(abi_file, json_string)?;
    
    Ok(())
}

fn generate_abi_path_constant(module_name: &str) -> TokenStream2 {
    let module_name_lit = proc_macro2::Literal::string(module_name);
    
    quote! {
        pub const ABI_PATH: &str = concat!(env!("OUT_DIR"), "/calimero/abi/", #module_name_lit, ".json");
    }
} 