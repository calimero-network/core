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
mod reserved;
mod sanitizer;
mod state;

// todo! use referenced lifetimes everywhere

// todo! permit #[app::logic(crate = "calimero_sdk")]
#[proc_macro_attribute]
pub fn logic(_args: TokenStream, input: TokenStream) -> TokenStream {
    reserved::init();
    let block = syn::parse_macro_input!(input as syn::ItemImpl);
    let tokens = match logic::LogicImpl::try_from(logic::LogicImplInput { item: &block }) {
        Ok(data) => data.to_token_stream(),
        Err(err) => err.to_compile_error(),
    };
    tokens.into()
}

#[proc_macro_attribute]
pub fn state(args: TokenStream, input: TokenStream) -> TokenStream {
    reserved::init();

    let input = input.into();

    let args = match syn::parse2(args.into()) {
        Ok(args) => args,
        Err(err) => {
            let err = err.to_compile_error();
            return quote!(#input #err).into();
        }
    };

    let item = match syn::parse2(input) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error().into(),
    };

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
pub fn destroy(_args: TokenStream, input: TokenStream) -> TokenStream {
    // this is a no-op, the attribute is just a marker
    input
}

#[proc_macro_attribute]
pub fn event(_args: TokenStream, input: TokenStream) -> TokenStream {
    reserved::init();
    let item = syn::parse_macro_input!(input as items::StructOrEnumItem);
    let tokens = match event::EventImpl::try_from(event::EventImplInput { item: &item }) {
        Ok(data) => data.to_token_stream(),
        Err(err) => err.to_compile_error(),
    };
    tokens.into()
}

#[proc_macro]
pub fn emit(input: TokenStream) -> TokenStream {
    // dbg!(input);
    TokenStream::new()
}
