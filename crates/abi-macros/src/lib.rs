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

use proc_macro::TokenStream;

/// Module-level ABI generation macro
#[proc_macro_attribute]
pub fn module(attr: TokenStream, item: TokenStream) -> TokenStream {
    module::module_impl(attr, item)
}

/// Query function marker
#[proc_macro_attribute]
pub fn query(attr: TokenStream, item: TokenStream) -> TokenStream {
    function::query_impl(attr, item)
}

/// Command function marker
#[proc_macro_attribute]
pub fn command(attr: TokenStream, item: TokenStream) -> TokenStream {
    function::command_impl(attr, item)
}

/// Event marker
#[proc_macro_attribute]
pub fn event(attr: TokenStream, item: TokenStream) -> TokenStream {
    event::event_impl(attr, item)
}

/// Derive macro for AbiType
#[proc_macro_derive(AbiType)]
pub fn derive_abi_type(input: TokenStream) -> TokenStream {
    derive::derive_abi_type_impl(input)
} 