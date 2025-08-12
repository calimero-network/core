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

mod module;
mod function;
mod event;
mod derive;
mod types;
mod wrappers;

use proc_macro::TokenStream;

// Wrapper macros that forward to existing SSApp macros
// These are the public API that apps should use

/// Wrapper for app::logic macro
#[proc_macro_attribute]
pub fn logic(attr: TokenStream, item: TokenStream) -> TokenStream {
    wrappers::logic_wrapper(attr, item)
}

/// Wrapper for app::state macro
#[proc_macro_attribute]
pub fn state(attr: TokenStream, item: TokenStream) -> TokenStream {
    wrappers::state_wrapper(attr, item)
}

/// Wrapper for app::init macro
#[proc_macro_attribute]
pub fn init(attr: TokenStream, item: TokenStream) -> TokenStream {
    wrappers::init_wrapper(attr, item)
}

/// Wrapper for app::destroy macro
#[proc_macro_attribute]
pub fn destroy(attr: TokenStream, item: TokenStream) -> TokenStream {
    wrappers::destroy_wrapper(attr, item)
}

/// Wrapper for app::event macro
#[proc_macro_attribute]
pub fn event(attr: TokenStream, item: TokenStream) -> TokenStream {
    wrappers::event_wrapper(attr, item)
}

// Hidden ABI-specific macros (not part of public API)
#[doc(hidden)]
#[proc_macro_attribute]
pub fn module(attr: TokenStream, item: TokenStream) -> TokenStream {
    module::module_impl(attr, item)
}

#[doc(hidden)]
#[proc_macro_attribute]
pub fn query(attr: TokenStream, item: TokenStream) -> TokenStream {
    function::query_impl(attr, item)
}

#[doc(hidden)]
#[proc_macro_attribute]
pub fn command(attr: TokenStream, item: TokenStream) -> TokenStream {
    function::command_impl(attr, item)
}

#[doc(hidden)]
#[proc_macro_attribute]
pub fn abi_event(attr: TokenStream, item: TokenStream) -> TokenStream {
    event::event_impl(attr, item)
}

/// Derive macro for AbiType
#[proc_macro_derive(AbiType)]
pub fn derive_abi_type(input: TokenStream) -> TokenStream {
    derive::derive_abi_type_impl(input)
}



 