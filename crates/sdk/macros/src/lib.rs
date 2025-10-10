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

    // Extract the type name from the impl block
    let type_name = &block.self_ty;
    let (impl_generics, ty_generics, where_clause) = block.generics.split_for_impl();

    let process_remote_events_method = quote! {
        /// Process remote events for automatic callbacks
        ///
        /// Uses the `#[derive(CallbackHandlers)]` dispatcher generated from the `Event` enum
        /// to decode and call the appropriate per-variant handler implemented on `self`.
        /// This method is generated when `#[app::callbacks]` is used.
        pub fn process_remote_events(&mut self, event_kind: ::std::string::String, event_data: ::std::string::String) -> ::calimero_sdk::app::Result<()> {
            // The event_data comes as base58-encoded string, so we need to decode it
            let decoded_event_data = ::bs58::decode(&event_data)
                .into_vec()
                .map_err(|_| ::calimero_sdk::types::Error::msg("invalid base58 event data"))?;

            // Use the event type from the AppState trait to dispatch events
            <#type_name as ::calimero_sdk::state::AppState>::Event::dispatch(self, &event_kind, &decoded_event_data)
        }
    };

    quote! {
        #block

        impl #impl_generics #type_name #ty_generics #where_clause {
            #process_remote_events_method
        }
    }
    .into()
}
