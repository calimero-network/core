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
use syn::{parse_macro_input, ItemImpl, ItemStruct, ItemEnum, AttributeArgs, parse::Parse, parse::ParseStream, Ident, Token, LitStr, ImplItem};
use std::collections::HashMap;
use sha2::{Digest, Sha256};

// Global state to collect ABI information during compilation
thread_local! {
    static ABI_COLLECTOR: std::cell::RefCell<AbiCollector> = std::cell::RefCell::new(AbiCollector::new());
}

#[derive(Debug, Default)]
struct AbiCollector {
    module_name: Option<String>,
    module_version: Option<String>,
    functions: Vec<FunctionInfo>,
    events: Vec<EventInfo>,
}

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

#[derive(Debug)]
struct EventInfo {
    name: String,
}

impl AbiCollector {
    fn new() -> Self {
        Self::default()
    }
    
    fn set_module_info(&mut self, name: String, version: String) {
        self.module_name = Some(name);
        self.module_version = Some(version);
    }
    
    fn add_function(&mut self, func: FunctionInfo) {
        self.functions.push(func);
    }
    
    fn add_event(&mut self, event: EventInfo) {
        self.events.push(event);
    }
    
    fn generate_abi_json(&self) -> Option<serde_json::Value> {
        let module_name = self.module_name.as_ref()?;
        let module_version = self.module_version.as_ref()?;
        
        // Generate source hash
        let mut hasher = sha2::Sha256::new();
        hasher.update(format!("{}{}", module_name, module_version).as_bytes());
        let source_hash = hex::encode(hasher.finalize());
        
        let mut abi = serde_json::json!({
            "metadata": {
                "schema_version": "0.1.0",
                "toolchain_version": "1.85.0",
                "source_hash": source_hash
            },
            "module_name": module_name,
            "module_version": module_version,
            "functions": {},
            "events": {}
        });
        
        // Add functions
        for func in &self.functions {
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
        for event in &self.events {
            abi["events"][&event.name] = serde_json::json!({
                "name": event.name,
                "payload_type": null
            });
        }
        
        Some(abi)
    }
}

/// Wrapper for app::logic macro
pub fn logic_wrapper(attr: TokenStream, item: TokenStream) -> TokenStream {
    // Forward to the original SDK macro
    let attr_tokens = TokenStream2::from(attr);
    let item_tokens = TokenStream2::from(item);
    
    let expanded = quote! {
        #[::calimero_sdk_macros::logic(#attr_tokens)]
        #item_tokens
    };
    
    expanded.into()
}

/// Wrapper for app::state macro
pub fn state_wrapper(attr: TokenStream, item: TokenStream) -> TokenStream {
    // Forward to the original SDK macro
    let attr_tokens = TokenStream2::from(attr);
    let item_tokens = TokenStream2::from(item);
    
    let expanded = quote! {
        #[::calimero_sdk_macros::state(#attr_tokens)]
        #item_tokens
    };
    
    expanded.into()
}

/// Wrapper for app::init macro
pub fn init_wrapper(attr: TokenStream, item: TokenStream) -> TokenStream {
    // Forward to the original SDK macro
    let attr_tokens = TokenStream2::from(attr);
    let item_tokens = TokenStream2::from(item);
    
    let expanded = quote! {
        #[::calimero_sdk_macros::init(#attr_tokens)]
        #item_tokens
    };
    
    expanded.into()
}

/// Wrapper for app::destroy macro
pub fn destroy_wrapper(attr: TokenStream, item: TokenStream) -> TokenStream {
    // Forward to the original SDK macro
    let attr_tokens = TokenStream2::from(attr);
    let item_tokens = TokenStream2::from(item);
    
    let expanded = quote! {
        #[::calimero_sdk_macros::destroy(#attr_tokens)]
        #item_tokens
    };
    
    expanded.into()
}

/// Wrapper for app::event macro
pub fn event_wrapper(attr: TokenStream, item: TokenStream) -> TokenStream {
    // Forward to the original SDK macro
    let attr_tokens = TokenStream2::from(attr);
    let item_tokens = TokenStream2::from(item);
    
    let expanded = quote! {
        #[::calimero_sdk_macros::event(#attr_tokens)]
        #item_tokens
    };
    
    expanded.into()
}

#[cfg(feature = "abi-export")]
fn collect_logic_functions(impl_block: &ItemImpl) {
    ABI_COLLECTOR.with(|collector| {
        let mut collector = collector.borrow_mut();
        
        for item in &impl_block.items {
            if let ImplItem::Fn(func) = item {
                // Check if function is public
                if !matches!(func.vis, syn::Visibility::Public(_)) {
                    continue;
                }
                
                // Check for init attribute
                let is_init = func.attrs.iter().any(|attr| {
                    attr.path().segments.last().map(|s| s.ident == "init").unwrap_or(false)
                });
                
                let kind = if is_init {
                    FunctionKind::Command // init is a command
                } else {
                    // Default to command for now, could be refined based on function signature
                    FunctionKind::Command
                };
                
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
                
                collector.add_function(FunctionInfo {
                    name,
                    kind,
                    parameters,
                    return_type,
                });
            }
        }
    });
}

#[cfg(feature = "abi-export")]
fn collect_event_info(item: &syn::Item) {
    ABI_COLLECTOR.with(|collector| {
        let mut collector = collector.borrow_mut();
        
        match item {
            syn::Item::Struct(struct_item) => {
                collector.add_event(EventInfo {
                    name: struct_item.ident.to_string(),
                });
            }
            syn::Item::Enum(enum_item) => {
                collector.add_event(EventInfo {
                    name: enum_item.ident.to_string(),
                });
            }
            _ => {}
        }
    });
}

#[cfg(feature = "abi-export")]
fn type_to_string(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Path(type_path) => {
            let mut segments = Vec::new();
            for segment in &type_path.path.segments {
                segments.push(segment.ident.to_string());
            }
            segments.join("::")
        }
        syn::Type::Reference(type_ref) => {
            let mut result = type_to_string(&type_ref.elem);
            if type_ref.mutability.is_some() {
                result = format!("&mut {}", result);
            } else {
                result = format!("&{}", result);
            }
            result
        }
        syn::Type::Slice(type_slice) => {
            format!("[{}]", type_to_string(&type_slice.elem))
        }
        syn::Type::Array(type_array) => {
            format!("[{}; {}]", type_to_string(&type_array.elem), type_array.len.to_token_stream())
        }
        syn::Type::Tuple(type_tuple) => {
            let elements: Vec<String> = type_tuple.elems.iter().map(type_to_string).collect();
            format!("({})", elements.join(", "))
        }
        _ => "unknown".to_string(),
    }
}

/// Emit the collected ABI if the feature is enabled
#[cfg(feature = "abi-export")]
pub fn emit_abi_if_enabled(module_name: &str) {
    ABI_COLLECTOR.with(|collector| {
        let collector = collector.borrow();
        if let Some(abi_json) = collector.generate_abi_json() {
            if let Ok(json_bytes) = serde_json::to_vec_pretty(&abi_json) {
                let _ = abi_core::build::emit_if_enabled(module_name, &json_bytes);
            }
        }
    });
}

#[cfg(not(feature = "abi-export"))]
pub fn emit_abi_if_enabled(_module_name: &str) {
    // No-op when feature is disabled
} 