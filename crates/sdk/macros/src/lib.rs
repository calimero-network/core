#![cfg_attr(
    all(test, feature = "nightly"),
    feature(non_exhaustive_omitted_patterns_lint)
)]

use proc_macro::TokenStream;
use quote::{quote, ToTokens};

mod errors;
mod event;
mod items;
mod logic;
mod macros;
mod reserved;
mod sanitizer;
mod state;

use macros::parse_macro_input;

// todo! use referenced lifetimes everywhere

// todo! permit #[app::logic(crate = "calimero_sdk")]
#[proc_macro_attribute]
pub fn logic(args: TokenStream, input: TokenStream) -> TokenStream {
    reserved::init();
    let _args = parse_macro_input!({ input } => args as items::Empty);
    let block = parse_macro_input!(input as syn::ItemImpl);

    // Find the #[app::init] method
    let init_method = match block.items.iter().find_map(|item| {
        if let syn::ImplItem::Fn(method) = item {
            if method.attrs.iter().any(is_app_init_attr) {
                Some(method)
            } else {
                None
            }
        } else {
            None
        }
    }) {
        Some(method) => method,
        None => {
            return syn::Error::new(
                proc_macro2::Span::call_site(),
                "An #[app::init] method is required",
            )
            .to_compile_error()
            .into()
        }
    };

    let tokens = match logic::LogicImpl::try_from(logic::LogicImplInput {
        item: &block,
        init_method,
    }) {
        Ok(data) => data.to_token_stream(),
        Err(err) => err.to_compile_error(),
    };

    tokens.into()
}

#[proc_macro_attribute]
pub fn state(args: TokenStream, input: TokenStream) -> TokenStream {
    reserved::init();

    let args = parse_macro_input!({ input } => args as state::StateArgs);
    let item = parse_macro_input!(input as items::StructOrEnumItem);

    let tokens = match state::StateImpl::try_from(state::StateImplInput {
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
    let _args = parse_macro_input!({ input } => args as items::Empty);
    let item = parse_macro_input!(input as items::StructOrEnumItem);
    let tokens = match event::EventImpl::try_from(event::EventImplInput { item: &item }) {
        Ok(data) => data.to_token_stream(),
        Err(err) => err.to_compile_error(),
    };
    tokens.into()
}

#[proc_macro]
pub fn emit(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::Expr);

    quote!(::calimero_sdk::event::emit(#input)).into()
}

fn is_app_init_attr(attr: &syn::Attribute) -> bool {
    let segments = &attr.path().segments;
    segments.len() == 2 && segments[0].ident == "app" && segments[1].ident == "init"
}
