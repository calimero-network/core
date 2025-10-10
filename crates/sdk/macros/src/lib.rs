#![cfg_attr(
    all(test, feature = "nightly"),
    feature(non_exhaustive_omitted_patterns_lint)
)]

use macros::parse_macro_input;
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{quote, ToTokens};
use syn::{Expr, ItemImpl};

use crate::event::{EventImpl, EventImplInput};
use crate::event_handlers::derive_callback_handlers;
use crate::items::{Empty, StructOrEnumItem};
use crate::logic::{LogicImpl, LogicImplInput};
use crate::private::{PrivateArgs, PrivateImpl, PrivateImplInput};
use crate::state::{StateArgs, StateImpl, StateImplInput};

mod errors;
mod event;
mod event_handlers;
mod items;
mod logic;
mod macros;
mod private;
mod reserved;
mod sanitizer;
mod state;

// todo! use referenced lifetimes everywhere

// todo! permit #[app::logic(crate = "calimero_sdk")]
#[proc_macro_attribute]
pub fn logic(args: TokenStream, input: TokenStream) -> TokenStream {
    reserved::init();
    let _args = parse_macro_input!({ input } => args as Empty);
    let block = parse_macro_input!(input as ItemImpl);

    let tokens = match LogicImpl::try_from(LogicImplInput { item: &block }) {
        Ok(data) => data.to_token_stream(),
        Err(err) => err.to_compile_error(),
    };

    tokens.into()
}

#[proc_macro_attribute]
pub fn state(args: TokenStream, input: TokenStream) -> TokenStream {
    reserved::init();

    let args = parse_macro_input!({ input } => args as StateArgs);
    let item = parse_macro_input!(input as StructOrEnumItem);

    let tokens = match StateImpl::try_from(StateImplInput {
        item: &item,
        args: &args,
    }) {
        Ok(data) => data.to_token_stream(),
        Err(err) => err.to_compile_error(),
    };

    tokens.into()
}

#[proc_macro_attribute]
pub fn private(args: TokenStream, input: TokenStream) -> TokenStream {
    reserved::init();

    let args = parse_macro_input!({ input } => args as PrivateArgs);
    let item = parse_macro_input!(input as StructOrEnumItem);

    let tokens = match PrivateImpl::try_from(PrivateImplInput {
        item: &item,
        args: &args,
    }) {
        Ok(data) => data.to_token_stream(),
        Err(err) => err.to_compile_error(),
    };

    tokens.into()
}

#[proc_macro_attribute]
pub fn init(_args: TokenStream, input: TokenStream) -> TokenStream {
    // this is a no-op, the attribute is just a marker
    input
}

#[proc_macro_attribute]
pub fn destroy(_args: TokenStream, input: TokenStream) -> TokenStream {
    // this is a no-op, the attribute is just a marker
    input
}

#[proc_macro_attribute]
pub fn event(args: TokenStream, input: TokenStream) -> TokenStream {
    reserved::init();
    let _args = parse_macro_input!({ input } => args as Empty);
    let item = parse_macro_input!(input as StructOrEnumItem);
    let tokens = match EventImpl::try_from(EventImplInput { item: &item }) {
        Ok(data) => data.to_token_stream(),
        Err(err) => err.to_compile_error(),
    };
    tokens.into()
}

#[proc_macro]
pub fn emit(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as Expr);

    quote!(::calimero_sdk::event::emit(#input)).into()
}

#[proc_macro]
pub fn err(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as TokenStream2);

    quote!(::calimero_sdk::__err__!(#input)).into()
}

#[proc_macro]
pub fn bail(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as TokenStream2);

    quote!(::calimero_sdk::__bail__!(#input)).into()
}

#[proc_macro]
pub fn log(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as TokenStream2);

    quote!(::calimero_sdk::__log__!(#input)).into()
}

#[proc_macro_derive(CallbackHandlers)]
pub fn derive_callback_handlers_macro(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::DeriveInput);
    let tokens = derive_callback_handlers(input);
    tokens.into()
}

#[proc_macro_attribute]
pub fn callbacks(args: TokenStream, input: TokenStream) -> TokenStream {
    reserved::init();
    let _args = parse_macro_input!({ input } => args as Empty);
    let block = parse_macro_input!(input as ItemImpl);

    // We don't need to extract generics since we're modifying the existing impl block

    let process_remote_events_method = quote! {
        /// Process remote events for automatic callbacks
        ///
        /// Uses the `#[derive(CallbackHandlers)]` dispatcher generated from the `Event` enum
        /// to decode and call the appropriate per-variant handler implemented on `self`.
        /// This method is generated when `#[app::callbacks]` is used.
        pub fn process_remote_events(&mut self, event_kind: ::std::string::String, event_data: ::std::vec::Vec<u8>) -> ::calimero_sdk::app::Result<()> {
            // Use the Event type directly to dispatch events
            // We need to use the concrete Event type, not the associated type from AppState
            // The Event type should be accessible in the same scope
            crate::Event::dispatch(self, &event_kind, &event_data)
        }
    };

    // Instead of creating a new impl block, we need to add the method to the existing block
    // Parse the impl block and add the method to it
    let mut new_block = block.clone();
    
    // Parse the method as an ImplItem
    let method_item: syn::ImplItem = syn::parse2(process_remote_events_method).unwrap();
    
    // Add the method to the impl block
    new_block.items.push(method_item);
    
    quote! {
        #new_block
    }
    .into()
}
