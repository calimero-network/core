#![cfg_attr(
    all(test, feature = "nightly"),
    feature(non_exhaustive_omitted_patterns_lint)
)]

use macros::parse_macro_input;
use proc_macro::TokenStream;
use quote::{quote, ToTokens};
use syn::{Expr, ItemImpl};

use crate::event::{EventImpl, EventImplInput};
use crate::items::{Empty, StructOrEnumItem};
use crate::logic::{LogicImpl, LogicImplInput};
use crate::state::{StateArgs, StateImpl, StateImplInput};

mod errors;
mod event;
mod items;
mod logic;
mod macros;
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
