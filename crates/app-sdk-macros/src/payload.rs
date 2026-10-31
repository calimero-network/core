//! `#[app::error]` and `#[app::event]`: enums whose variants carry JSON payloads.
//!
//! The variant name is the error code or event name. Fields are camelCase JSON keys, and
//! every field is named (D63): a tuple variant, or a tuple struct in place of the enum, is a
//! compile error; a unit variant has no fields. `u64`/`i64` fields are decimal strings (D20,
//! Ruling 25). Each macro also emits its part of the ABI as a module-level constant that
//! `#[app::logic]` in the same module picks up.

use proc_macro2::TokenStream;
use quote::{quote, ToTokens};
use serde_json::{Map, Value};
use syn::{Fields, Item, ItemEnum};

use crate::types::{abi_type, camel, canonical, check_wire, wire, Wire};

/// What the payload enum becomes.
#[derive(Clone, Copy)]
pub enum Payload {
    Error,
    Event,
}

impl Payload {
    fn noun(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Event => "event",
        }
    }
}

/// The refusal of fields without names (D63), pointing at `at`.
fn unnamed(kind: Payload, at: impl ToTokens, what: &str, name: &str) -> syn::Error {
    syn::Error::new_spanned(
        at,
        format!(
            "app {}s name every field (D63): {what} `{name}` has unnamed fields; write `{name} {{ field: Type }}`",
            kind.noun()
        ),
    )
}

/// `#[app::error]` / `#[app::event]` on any item: an enum is expanded, a tuple struct is
/// refused naming D63, anything else is refused as not an enum.
pub fn expand_item(kind: Payload, item: &Item) -> syn::Result<TokenStream> {
    match item {
        Item::Enum(item) => expand(kind, item),
        Item::Struct(s) if matches!(s.fields, Fields::Unnamed(_)) => {
            Err(unnamed(kind, s, "tuple struct", &s.ident.to_string()))
        }
        other => Err(syn::Error::new_spanned(
            other,
            format!(
                "app {}s are an enum: one variant per {}, each with named fields",
                kind.noun(),
                kind.noun()
            ),
        )),
    }
}

pub fn expand(kind: Payload, item: &ItemEnum) -> syn::Result<TokenStream> {
    if !item.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.generics,
            "app errors and events take no generics",
        ));
    }
    let name = &item.ident;
    let mut abi = Map::new();
    let mut arms = Vec::new();
    for variant in &item.variants {
        let vname = &variant.ident;
        // A unit variant or a braced one; a tuple variant is refused (D63).
        let named = match &variant.fields {
            Fields::Unnamed(_) => {
                return Err(unnamed(kind, variant, "tuple variant", &vname.to_string()))
            }
            Fields::Unit => None,
            Fields::Named(fields) => Some(fields),
        };
        let code = vname.to_string();
        let mut fields_abi = Map::new();
        let mut binds = Vec::new();
        let mut inserts = Vec::new();
        // Every field of a braced variant has its name.
        let fields = named
            .into_iter()
            .flat_map(|n| &n.named)
            .filter_map(|f| f.ident.clone().map(|ident| (f, ident)));
        for (field, bind) in fields {
            let key = camel(&bind.to_string());
            check_wire(&field.ty)?;
            let _ = fields_abi.insert(key.clone(), Value::String(abi_type(&field.ty)));
            let value = match wire(&field.ty) {
                Some(Wire::Bytes) => quote! {
                    ::calimero_app_sdk::__private::serde_json::Value::String(
                        ::calimero_app_sdk::__private::base64url(#bind),
                    )
                },
                Some(Wire::OptBytes) => quote! {
                    match #bind {
                        ::core::option::Option::Some(v) => {
                            ::calimero_app_sdk::__private::serde_json::Value::String(
                                ::calimero_app_sdk::__private::base64url(v),
                            )
                        }
                        ::core::option::Option::None => {
                            ::calimero_app_sdk::__private::serde_json::Value::Null
                        }
                    }
                },
                Some(Wire::U64 | Wire::I64) => quote! {
                    ::calimero_app_sdk::__private::serde_json::Value::String(#bind.to_string())
                },
                Some(Wire::OptU64 | Wire::OptI64) => quote! {
                    match #bind {
                        ::core::option::Option::Some(v) => {
                            ::calimero_app_sdk::__private::serde_json::Value::String(v.to_string())
                        }
                        ::core::option::Option::None => {
                            ::calimero_app_sdk::__private::serde_json::Value::Null
                        }
                    }
                },
                None => quote! { ::calimero_app_sdk::__private::to_json_value(#bind)? },
            };
            inserts.push(quote! { let _ = __map.insert(#key.into(), #value); });
            binds.push(bind);
        }
        let _ = abi.insert(code.clone(), Value::Object(fields_abi));
        let pattern = match named {
            None => quote! { #name::#vname },
            Some(_) => quote! { #name::#vname { #(#binds),* } },
        };
        arms.push(quote! {
            #pattern => {
                #[allow(unused_mut)]
                let mut __map = ::calimero_app_sdk::__private::serde_json::Map::new();
                #(#inserts)*
                (#code, __map)
            }
        });
    }
    let abi = canonical(&Value::Object(abi));
    let (konst, trait_impl) = match kind {
        Payload::Error => (
            quote!(__CALIMERO_ABI_ERRORS),
            quote! {
                impl ::core::convert::From<#name> for ::calimero_app_sdk::AppError {
                    fn from(e: #name) -> Self {
                        match ::calimero_app_sdk::__private::payload(&e, |e| {
                            ::core::result::Result::Ok(match e { #(#arms)* })
                        }) {
                            ::core::result::Result::Ok((code, fields)) => {
                                ::calimero_app_sdk::AppError::with_fields(code, fields)
                            }
                            ::core::result::Result::Err(err) => err,
                        }
                    }
                }
            },
        ),
        Payload::Event => (
            quote!(__CALIMERO_ABI_EVENTS),
            quote! {
                impl ::calimero_app_sdk::app::AppEvent for #name {
                    fn encode(
                        &self,
                    ) -> ::calimero_app_sdk::Result<(&'static str, ::calimero_app_sdk::__private::serde_json::Value)> {
                        ::calimero_app_sdk::__private::payload(self, |e| {
                            ::core::result::Result::Ok(match e { #(#arms)* })
                        })
                    }
                }
            },
        ),
    };
    Ok(quote! {
        #item

        #trait_impl

        /// This module's ABI part, read by `#[app::logic]`.
        #[doc(hidden)]
        #[allow(non_upper_case_globals, dead_code)]
        pub const #konst: &str = #abi;
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    #[test]
    fn payload_abi_is_sorted_whatever_serde_json_features() {
        let item: ItemEnum = parse_quote! {
            pub enum E { Zeta { b_field: u32, a_field: String, data: Vec<u8> }, Alpha }
        };
        let out = expand(Payload::Event, &item).unwrap().to_string();
        let abi = r#"{"Alpha":{},"Zeta":{"aField":"string","bField":"u32","data":"bytes"}}"#;
        let lit = proc_macro2::Literal::string(abi).to_string();
        assert!(out.contains(&lit), "{out}");
    }

    #[test]
    fn tuple_variants_are_refused_naming_d63() {
        for kind in [Payload::Error, Payload::Event] {
            let item: ItemEnum =
                parse_quote! { pub enum E { Ok { a: u32 }, Invalid(String), Gone } };
            let err = expand(kind, &item).expect_err("refused");
            let msg = err.to_string();
            assert!(msg.contains("D63") && msg.contains("`Invalid`"), "{msg}");
        }
    }

    #[test]
    fn a_tuple_struct_is_refused_naming_d63() {
        let item: Item = parse_quote! { pub struct Booked(String, u32); };
        let err = expand_item(Payload::Event, &item).expect_err("refused");
        assert!(err.to_string().contains("D63"), "{err}");
        let item: Item = parse_quote! { pub struct Booked { id: String } };
        let err = expand_item(Payload::Event, &item).expect_err("refused");
        assert!(err.to_string().contains("an enum"), "{err}");
    }

    #[test]
    fn unit_and_named_variants_are_kept() {
        let item: ItemEnum = parse_quote! { pub enum E { Gone, Taken { start: u64 } } };
        let out = expand(Payload::Error, &item).unwrap().to_string();
        let abi = r#"{"Gone":{},"Taken":{"start":"u64"}}"#;
        let lit = proc_macro2::Literal::string(abi).to_string();
        assert!(out.contains(&lit), "{out}");
    }

    #[test]
    fn nested_wide_integers_are_refused() {
        let item: ItemEnum = parse_quote! { pub enum E { A { xs: Vec<u64> } } };
        let err = expand(Payload::Error, &item).expect_err("refused");
        assert!(err.to_string().contains("D20"), "{err}");
    }
}
