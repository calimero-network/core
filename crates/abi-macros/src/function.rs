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
use quote::quote;
use syn::{parse_macro_input, ItemFn, Visibility, FnArg, Type};

pub fn query_impl(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let func = parse_macro_input!(item as ItemFn);
    
    // Validate function is public
    if !matches!(func.vis, Visibility::Public(_)) {
        return syn::Error::new_spanned(&func.sig, "query functions must be public")
            .to_compile_error()
            .into();
    }
    
    // Validate parameter types
    if let Err(e) = validate_function_parameters(&func.sig.inputs.iter().collect::<Vec<_>>()) {
        return e.to_compile_error().into();
    }
    
    // For now, just pass through the function unchanged
    // The module macro will collect the function information
    let expanded = quote! {
        #func
    };
    
    expanded.into()
}

fn validate_function_parameters(inputs: &[&FnArg]) -> syn::Result<()> {
    for input in inputs {
        if let FnArg::Typed(pat_type) = input {
            if !is_supported_type(&pat_type.ty) {
                return Err(syn::Error::new_spanned(&pat_type.ty, 
                    format!("unsupported parameter type. Supported types: String, u8, u16, u32, u64, u128, i8, i16, i32, i64, i128, bool, Vec<T> where T is supported")));
            }
        }
    }
    Ok(())
}

fn is_supported_type(ty: &Type) -> bool {
    match ty {
        Type::Path(type_path) => {
            let path = &type_path.path;
            if path.segments.len() == 1 {
                let segment = &path.segments[0];
                match segment.ident.to_string().as_str() {
                    "String" | "u8" | "u16" | "u32" | "u64" | "u128" | 
                    "i8" | "i16" | "i32" | "i64" | "i128" | "bool" => true,
                    _ => false,
                }
            } else if path.segments.len() == 2 && path.segments[0].ident == "Vec" {
                // Vec<T> is supported if T is supported
                true // For now, assume Vec<T> is always supported
            } else {
                false
            }
        }
        _ => false,
    }
}

pub fn command_impl(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let func = parse_macro_input!(item as ItemFn);
    
    // Validate function is public
    if !matches!(func.vis, Visibility::Public(_)) {
        return syn::Error::new_spanned(&func.sig, "command functions must be public")
            .to_compile_error()
            .into();
    }
    
    // Validate parameter types
    if let Err(e) = validate_function_parameters(&func.sig.inputs.iter().collect::<Vec<_>>()) {
        return e.to_compile_error().into();
    }
    
    // For now, just pass through the function unchanged
    // The module macro will collect the function information
    let expanded = quote! {
        #func
    };
    
    expanded.into()
} 