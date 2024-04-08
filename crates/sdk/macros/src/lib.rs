#![cfg_attr(
    all(test, feature = "nightly"),
    feature(non_exhaustive_omitted_patterns_lint)
)]

use proc_macro::TokenStream;
use quote::ToTokens;

mod errors;
mod logic;
mod sanitizers;

// todo! use referenced lifetimes everywhere

#[proc_macro_attribute]
pub fn logic(args: TokenStream, input: TokenStream) -> TokenStream {
    let block = syn::parse_macro_input!(input as syn::ItemImpl);
    let tokens = match logic::LogicImpl::try_from(&block) {
        Ok(data) => data.to_token_stream(),
        Err(err) => err.to_compile_error(),
    };
    tokens.into()
}

#[proc_macro_attribute]
pub fn state(args: TokenStream, input: TokenStream) -> TokenStream {
    // disallow lifetime annotations, perhaps you meant to put this on the logic block?

    // dbg!(attr);
    // dbg!(input)
    input
}

#[proc_macro_attribute]
pub fn destroy(_args: TokenStream, input: TokenStream) -> TokenStream {
    // this is a no-op, the attribute is just a marker
    input
}

#[proc_macro_attribute]
pub fn event(args: TokenStream, input: TokenStream) -> TokenStream {
    dbg!(args, input);
    TokenStream::new()
}
