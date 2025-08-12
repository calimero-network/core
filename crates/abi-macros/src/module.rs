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
use syn::{parse_macro_input, AttributeArgs, ItemMod, Item, Visibility, parse::Parse, parse::ParseStream, Ident, Token, LitStr};
use std::fs;
use std::path::Path;
use sha2::{Digest, Sha256};

#[cfg(feature = "abi-export")]
use abi_core;


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
    return_type: Option<String>,
}

#[derive(Debug)]
enum FunctionKind {
    Query,
    Command,
}

#[derive(Debug)]
struct ParameterInfo {
    name: String,
    ty: String,
}

/// Event information collected from module
#[derive(Debug)]
struct EventInfo {
    name: String,
}

pub fn module_impl(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = parse_macro_input!(attr as AttributeArgs);
    let item = parse_macro_input!(item as ItemMod);
    
    // Parse attributes
    let attrs = match parse_module_attrs(&attr) {
        Ok(attrs) => attrs,
        Err(e) => return e.to_compile_error().into(),
    };
    
    // Collect functions and events from module
    let mut functions = Vec::new();
    let mut events = Vec::new();
    
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
                }
                _ => {}
            }
        }
    }
    

    
    // Generate ABI JSON
    let abi_json = generate_abi_json(&attrs, &functions, &events);
    
    // Write ABI file using the new build system
    #[cfg(feature = "abi-export")]
    {
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
    
    // Extract function signature
    let name = func.sig.ident.to_string();
    let mut parameters = Vec::new();
    
    for param in &func.sig.inputs {
        if let syn::FnArg::Typed(pat_type) = param {
            if let syn::Pat::Ident(pat_ident) = &*pat_type.pat {
                let param_name = pat_ident.ident.to_string();
                let param_ty = type_to_string(&pat_type.ty);
                parameters.push(ParameterInfo {
                    name: param_name,
                    ty: param_ty,
                });
            }
        }
    }
    
    let return_type = match &func.sig.output {
        syn::ReturnType::Default => Some("()".to_string()),
        syn::ReturnType::Type(_, ty) => Some(type_to_string(&ty)),
    };
    
    Some(FunctionInfo {
        name,
        kind,
        parameters,
        return_type,
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
            });
        }
    }
    
    None
}

fn type_to_string(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Path(type_path) => {
            let mut segments = Vec::new();
            for segment in &type_path.path.segments {
                segments.push(segment.ident.to_string());
            }
            segments.join("::")
        }
        _ => "unknown".to_string(),
    }
}

fn generate_abi_json(attrs: &ModuleAttrs, functions: &[FunctionInfo], events: &[EventInfo]) -> serde_json::Value {
    // Generate source hash (simplified for PR1)
    let mut hasher = Sha256::new();
    hasher.update(format!("{}{}", attrs.name, attrs.version).as_bytes());
    let source_hash = hex::encode(hasher.finalize());
    
    let mut abi = serde_json::json!({
        "metadata": {
            "schema_version": "0.1.0",
            "toolchain_version": "1.85.0",
            "source_hash": source_hash
        },
        "module_name": attrs.name,
        "module_version": attrs.version,
        "functions": {},
        "events": {}
    });
    
    // Add functions
    for func in functions {
        let kind = match func.kind {
            FunctionKind::Query => "query",
            FunctionKind::Command => "command",
        };
        
        let mut func_json = serde_json::json!({
            "name": func.name,
            "kind": kind,
            "parameters": []
        });
        
        for param in &func.parameters {
            func_json["parameters"].as_array_mut().unwrap().push(
                serde_json::json!({
                    "name": param.name,
                    "ty": param.ty,
                    "direction": "input"
                })
            );
        }
        
        if let Some(ret_ty) = &func.return_type {
            func_json["return_type"] = serde_json::json!(ret_ty);
        }
        
        abi["functions"][&func.name] = func_json;
    }
    
    // Add events
    for event in events {
        abi["events"][&event.name] = serde_json::json!({
            "name": event.name,
            "payload_type": null
        });
    }
    
    abi
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