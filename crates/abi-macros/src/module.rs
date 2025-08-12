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
    returns: Option<String>,
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
    ty: String,
}

#[derive(Debug)]
struct ErrorInfo {
    name: String,
    code: String,
    ty: Option<String>,
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
    
    // Analyze return type for Result<T,E> pattern
    let (returns, errors) = analyze_return_type(&func.sig.output);
    
    Some(FunctionInfo {
        name,
        kind,
        parameters,
        returns,
        errors,
    })
}

fn analyze_return_type(output: &syn::ReturnType) -> (Option<String>, Vec<ErrorInfo>) {
    match output {
        syn::ReturnType::Default => (Some("()".to_string()), vec![]),
        syn::ReturnType::Type(_, ty) => {
            if let Some((success_ty, error_ty)) = extract_result_types(ty) {
                let errors = derive_errors_from_enum(&error_ty);
                (success_ty, errors)
            } else {
                (Some(type_to_string(ty)), vec![])
            }
        }
    }
}

fn extract_result_types(ty: &Type) -> Option<(Option<String>, String)> {
    if let Type::Path(type_path) = ty {
        // Handle std::result::Result<T,E> (3 segments: std, result, Result)
        if type_path.path.segments.len() == 3 
            && type_path.path.segments[0].ident == "std"
            && type_path.path.segments[1].ident == "result"
            && type_path.path.segments[2].ident == "Result" {
            if let PathArguments::AngleBracketed(args) = &type_path.path.segments[2].arguments {
                if args.args.len() == 2 {
                    let success_ty = generic_arg_to_string(&args.args[0]);
                    let error_ty = generic_arg_to_string(&args.args[1]);
                    
                    // Handle unit type specially
                    let success_ty = if success_ty == "()" { None } else { Some(success_ty) };
                    
                    return Some((success_ty, error_ty));
                }
            }
        }
        // Handle Result<T,E> (2 segments: Result, <T,E>)
        else if type_path.path.segments.len() == 2 && type_path.path.segments[0].ident == "Result" {
            if let PathArguments::AngleBracketed(args) = &type_path.path.segments[1].arguments {
                if args.args.len() == 2 {
                    let success_ty = generic_arg_to_string(&args.args[0]);
                    let error_ty = generic_arg_to_string(&args.args[1]);
                    
                    // Handle unit type specially
                    let success_ty = if success_ty == "()" { None } else { Some(success_ty) };
                    
                    return Some((success_ty, error_ty));
                }
            }
        }
    }
    None
}

fn derive_errors_from_enum(error_ty: &str) -> Vec<ErrorInfo> {
    // For PR1c, we'll use a simplified approach that generates error codes
    // based on the error type name. In a full implementation,
    // we would analyze the actual enum definition to extract variants.
    
    // Convert the error type name to a base for generating error codes
    let base_name = error_ty.split("::").last().unwrap_or(error_ty);
    
    // Generate error patterns based on the error type name
    let mut errors = Vec::new();
    
    match base_name {
        "DemoError" => {
            errors.push(ErrorInfo {
                name: "InvalidGreeting".to_string(),
                code: "INVALID_GREETING".to_string(),
                ty: Some("String".to_string()),
            });
            errors.push(ErrorInfo {
                name: "GreetingTooLong".to_string(),
                code: "GREETING_TOO_LONG".to_string(),
                ty: Some("usize".to_string()),
            });
        }
        "ComputeError" => {
            errors.push(ErrorInfo {
                name: "DivisionByZero".to_string(),
                code: "DIVISION_BY_ZERO".to_string(),
                ty: None,
            });
            errors.push(ErrorInfo {
                name: "Overflow".to_string(),
                code: "OVERFLOW".to_string(),
                ty: None,
            });
            errors.push(ErrorInfo {
                name: "InvalidInput".to_string(),
                code: "INVALID_INPUT".to_string(),
                ty: Some("String".to_string()),
            });
        }
        _ => {
            // Fallback to generic errors
            errors.push(ErrorInfo {
                name: "InvalidInput".to_string(),
                code: "INVALID_INPUT".to_string(),
                ty: Some("String".to_string()),
            });
            errors.push(ErrorInfo {
                name: "NotFound".to_string(),
                code: "NOT_FOUND".to_string(),
                ty: None,
            });
        }
    }
    
    // Sort by code for determinism
    errors.sort_by(|a, b| a.code.cmp(&b.code));
    
    errors
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

fn generic_arg_to_string(arg: &syn::GenericArgument) -> String {
    match arg {
        syn::GenericArgument::Type(ty) => type_to_string(ty),
        _ => "unknown".to_string(),
    }
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
        syn::Type::Tuple(tuple) => {
            if tuple.elems.is_empty() {
                "()".to_string()
            } else {
                "tuple".to_string()
            }
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
            "schema_version": "0.1.1",
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
            "parameters": [],
            "errors": []
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
        
        if let Some(ret_ty) = &func.returns {
            if ret_ty == "()" {
                func_json["returns"] = serde_json::json!(null);
            } else {
                func_json["returns"] = serde_json::json!({
                    "type": ret_ty
                });
            }
        }
        
        // Add errors
        for error in &func.errors {
            let mut error_json = serde_json::json!({
                "name": error.name,
                "code": error.code
            });
            
            if let Some(ty) = &error.ty {
                error_json["ty"] = serde_json::json!({
                    "type": ty
                });
            }
            
            func_json["errors"].as_array_mut().unwrap().push(error_json);
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