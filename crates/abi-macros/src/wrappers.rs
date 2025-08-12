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

/// Wrapper for app::logic macro
pub fn logic_wrapper(attr: TokenStream, item: TokenStream) -> TokenStream {
    // Forward to the original SDK macro
    let attr_tokens = TokenStream2::from(attr);
    let item_tokens = TokenStream2::from(item);
    
    #[cfg(feature = "abi-export")]
    {
        // Generate ABI when feature is enabled
        let expanded = quote! {
            #[::calimero_sdk_macros::logic(#attr_tokens)]
            #item_tokens
            
            // ABI generation will be handled by the module macro
        };
        expanded.into()
    }
    
    #[cfg(not(feature = "abi-export"))]
    {
        let expanded = quote! {
            #[::calimero_sdk_macros::logic(#attr_tokens)]
            #item_tokens
        };
        expanded.into()
    }
}

/// Wrapper for app::state macro
pub fn state_wrapper(attr: TokenStream, item: TokenStream) -> TokenStream {
    // Forward to the original SDK macro
    let attr_tokens = TokenStream2::from(attr);
    let item_tokens = TokenStream2::from(item);
    
    #[cfg(feature = "abi-export")]
    {
        // Generate ABI when feature is enabled
        let expanded = quote! {
            #[::calimero_sdk_macros::state(#attr_tokens)]
            #item_tokens
            
            // ABI generation will be handled by the module macro
        };
        expanded.into()
    }
    
    #[cfg(not(feature = "abi-export"))]
    {
        let expanded = quote! {
            #[::calimero_sdk_macros::state(#attr_tokens)]
            #item_tokens
        };
        expanded.into()
    }
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