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
use syn::{parse_macro_input, ItemFn, Visibility};

pub fn query_impl(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let func = parse_macro_input!(item as ItemFn);
    
    // Validate function is public
    if !matches!(func.vis, Visibility::Public(_)) {
        return syn::Error::new_spanned(&func.sig, "query functions must be public")
            .to_compile_error()
            .into();
    }
    
    // For now, just pass through the function unchanged
    // The module macro will collect the function information
    let expanded = quote! {
        #func
    };
    
    expanded.into()
}

pub fn command_impl(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let func = parse_macro_input!(item as ItemFn);
    
    // Validate function is public
    if !matches!(func.vis, Visibility::Public(_)) {
        return syn::Error::new_spanned(&func.sig, "command functions must be public")
            .to_compile_error()
            .into();
    }
    
    // For now, just pass through the function unchanged
    // The module macro will collect the function information
    let expanded = quote! {
        #func
    };
    
    expanded.into()
} 