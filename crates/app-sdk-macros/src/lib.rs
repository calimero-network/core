//! The `app` attributes of `calimero-app-sdk`, re-exported there as `calimero_app_sdk::app`
//! (and `Record` from the prelude). Generated code names only `::calimero_app_sdk` paths, so
//! an app depends on the SDK alone.

use proc_macro::TokenStream;
use syn::{parse_macro_input, DeriveInput, ItemEnum, ItemImpl, ItemStruct};

mod logic;
mod payload;
mod record;
mod state;
mod types;

fn emit(result: syn::Result<proc_macro2::TokenStream>) -> TokenStream {
    result.unwrap_or_else(syn::Error::into_compile_error).into()
}

/// The state root: `#[app::state(version = N)] pub struct App { tables and collections }`.
#[proc_macro_attribute]
pub fn state(args: TokenStream, input: TokenStream) -> TokenStream {
    let item = parse_macro_input!(input as ItemStruct);
    emit(state::expand(args.into(), &item))
}

/// The methods block: generates the `handle` export, argument decoding and the ABI.
#[proc_macro_attribute]
pub fn logic(args: TokenStream, input: TokenStream) -> TokenStream {
    if !args.is_empty() {
        return syn::Error::new(
            proc_macro2::Span::call_site(),
            "#[app::logic] takes no arguments",
        )
        .into_compile_error()
        .into();
    }
    let item = parse_macro_input!(input as ItemImpl);
    emit(logic::expand(item))
}

/// The app's error enum: the variant name is the error code, its fields the error's fields.
#[proc_macro_attribute]
pub fn error(_args: TokenStream, input: TokenStream) -> TokenStream {
    let item = parse_macro_input!(input as ItemEnum);
    emit(payload::expand(payload::Payload::Error, &item))
}

/// The app's event enum: the variant name is the event name, its fields the event data.
#[proc_macro_attribute]
pub fn event(_args: TokenStream, input: TokenStream) -> TokenStream {
    let item = parse_macro_input!(input as ItemEnum);
    emit(payload::expand(payload::Payload::Event, &item))
}

/// A table record: `#[derive(Record)] #[table(key = "id", unique(..), index(..), count(..))]`.
#[proc_macro_derive(Record, attributes(table))]
pub fn derive_record(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    emit(record::expand(&input))
}

macro_rules! method_attribute {
    ($($name:ident),*) => {$(
        /// A method attribute; read by `#[app::logic]`, an error anywhere else.
        #[proc_macro_attribute]
        pub fn $name(_args: TokenStream, input: TokenStream) -> TokenStream {
            let mut out: TokenStream = syn::Error::new(
                proc_macro2::Span::call_site(),
                concat!("`app::", stringify!($name), "` goes on a method inside `#[app::logic]`"),
            )
            .into_compile_error()
            .into();
            out.extend(input);
            out
        }
    )*};
}

method_attribute!(init, view, confirm, captcha, public, every, allow);
