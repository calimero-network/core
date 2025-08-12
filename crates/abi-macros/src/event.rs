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
use syn::{parse_macro_input, ItemStruct, Visibility};

pub fn event_impl(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let struct_item = parse_macro_input!(item as ItemStruct);
    
    // Validate struct is public
    if !matches!(struct_item.vis, Visibility::Public(_)) {
        return syn::Error::new_spanned(&struct_item.ident, "event structs must be public")
            .to_compile_error()
            .into();
    }
    
    // For now, just pass through the struct unchanged
    // The module macro will collect the event information
    let expanded = quote! {
        #struct_item
    };
    
    expanded.into()
} 