//! The SDK crate generated code names: the one the app depends on.
//!
//! An app names the SDK as `calimero-sdk`, or, while apps move over, by the old name
//! `calimero-app-sdk` (a crate that only re-exports `calimero-sdk`), and may rename either
//! (`sdk = { package = "calimero-sdk", .. }`). Generated paths go through whichever the app's
//! manifest lists, so an app that depends on only one of the two builds.

use proc_macro2::{Span, TokenStream};
use proc_macro_crate::{crate_name, FoundCrate};
use quote::quote;

/// The SDK packages, in the order they are looked for.
const PACKAGES: [&str; 2] = ["calimero-sdk", "calimero-app-sdk"];

/// The crate name as code refers to it: the app's name for its SDK dependency, or the SDK's
/// own name inside the SDK (its tests and doctests; the SDK declares
/// `extern crate self as calimero_sdk`). An app that lists neither (its manifest unreadable,
/// say) gets `calimero_sdk`, whose error then names the missing dependency.
pub fn name() -> String {
    for package in PACKAGES {
        match crate_name(package) {
            Ok(FoundCrate::Itself) => return package.replace('-', "_"),
            Ok(FoundCrate::Name(name)) => return name,
            Err(_) => {}
        }
    }
    "calimero_sdk".to_owned()
}

/// Whether `ident` names the SDK in an attribute path (`<sdk>::app::view`): either
/// package name, or the app's own name for its dependency.
pub fn is_sdk(ident: &str) -> bool {
    ident == "calimero_sdk" || ident == "calimero_app_sdk" || ident == name()
}

/// `::<sdk>`, to splice before a path: `quote!(#sdk::AppError)`.
pub fn path() -> TokenStream {
    let ident = syn::Ident::new(&name(), Span::call_site());
    quote!(::#ident)
}

/// `"::<sdk><rest>"`, for paths an attribute takes as a string (`serde(crate = ..)`,
/// `serde(with = ..)`).
pub fn string(rest: &str) -> String {
    format!("::{}{rest}", name())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sdk_names_itself_by_its_new_name() {
        // These unit tests run in the macros crate, which lists neither package: the fallback.
        assert_eq!(name(), "calimero_sdk");
        assert_eq!(string("::json::u64_str"), "::calimero_sdk::json::u64_str");
    }
}
