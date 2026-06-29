use proc_macro2::TokenStream;
use quote::{quote, ToTokens};
use syn::{parse_quote, Error as SynError, GenericParam, Generics, Ident, Visibility};

use crate::errors::{Errors, ParseError};
use crate::items::StructOrEnumItem;
use crate::reserved::{idents, lifetimes};

pub struct EventImpl<'a> {
    ident: &'a Ident,
    generics: &'a Generics,
    orig: &'a StructOrEnumItem,
}

impl ToTokens for EventImpl<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let EventImpl {
            ident,
            generics: source_generics,
            orig,
        } = *self;

        let mut generics = source_generics.clone();

        for generic_ty in source_generics.type_params() {
            generics
                .make_where_clause()
                .predicates
                .push(parse_quote!(#generic_ty: ::calimero_sdk::serde::Serialize));
        }

        let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

        quote! {
            #[derive(::calimero_sdk::serde::Serialize)]
            #[serde(crate = "::calimero_sdk::serde")]
            #[serde(tag = "kind", content = "data")]
            #orig

            impl #impl_generics ::calimero_sdk::event::AppEvent for #ident #ty_generics #where_clause {
                // `kind` and `data` are read off the serde-tagged object so the
                // wire names honour any `#[serde(rename/rename_all)]` the variant
                // carries — deriving them from the raw variant identifier here
                // would silently diverge from how the event actually serializes.
                fn kind(&self) -> ::std::borrow::Cow<str> {
                    match ::calimero_sdk::serde_json::to_value(self) {
                        ::core::result::Result::Ok(
                            ::calimero_sdk::serde_json::Value::Object(mut __obj)
                        ) => match __obj.remove("kind") {
                            ::core::option::Option::Some(
                                ::calimero_sdk::serde_json::Value::String(__k)
                            ) => ::std::borrow::Cow::Owned(__k),
                            _ => ::calimero_sdk::env::panic_str(
                                "event did not serialize with a string `kind` tag",
                            ),
                        },
                        ::core::result::Result::Ok(_) => ::calimero_sdk::env::panic_str(
                            "event did not serialize to a `{ kind, data }` object",
                        ),
                        ::core::result::Result::Err(__err) => ::calimero_sdk::env::panic_str(
                            &::std::format!("Failed to serialize event: {:?}", __err)
                        ),
                    }
                }
                fn data(&self) -> ::std::borrow::Cow<[u8]> {
                    match ::calimero_sdk::serde_json::to_value(self) {
                        ::core::result::Result::Ok(
                            ::calimero_sdk::serde_json::Value::Object(__obj)
                        ) => match __obj.get("data") {
                            // A unit variant carries no `content`, so serde omits
                            // `data` (or leaves it null). Emit an empty payload —
                            // not the 4-byte JSON literal `null` the old
                            // round-trip produced.
                            ::core::option::Option::None
                            | ::core::option::Option::Some(
                                ::calimero_sdk::serde_json::Value::Null
                            ) => ::std::borrow::Cow::Borrowed(&[][..]),
                            ::core::option::Option::Some(__data) => {
                                match ::calimero_sdk::serde_json::to_vec(__data) {
                                    ::core::result::Result::Ok(__bytes) =>
                                        ::std::borrow::Cow::Owned(__bytes),
                                    ::core::result::Result::Err(__err) =>
                                        ::calimero_sdk::env::panic_str(
                                            &::std::format!(
                                                "Failed to serialize event data: {:?}", __err
                                            )
                                        ),
                                }
                            }
                        },
                        ::core::result::Result::Ok(_) => ::calimero_sdk::env::panic_str(
                            "event did not serialize to a `{ kind, data }` object",
                        ),
                        ::core::result::Result::Err(__err) => ::calimero_sdk::env::panic_str(
                            &::std::format!("Failed to serialize event: {:?}", __err)
                        ),
                    }
                }
            }

            impl #impl_generics ::calimero_sdk::event::AppEventExt for #ident #ty_generics #where_clause {}
        }
        .to_tokens(tokens);
    }
}

pub struct EventImplInput<'a> {
    pub item: &'a StructOrEnumItem,
}

impl<'a> TryFrom<EventImplInput<'a>> for EventImpl<'a> {
    type Error = Errors<'a, StructOrEnumItem>;

    fn try_from(input: EventImplInput<'a>) -> Result<Self, Self::Error> {
        let errors = Errors::new(input.item);

        let (vis, ident, generics) = match input.item {
            StructOrEnumItem::Struct(item) => {
                // A struct can't carry the `{ kind, data }` tagged-union shape an
                // event serializes to. Reject it here with a clear SDK message,
                // otherwise the `#[serde(content = "...")]` we generate below
                // surfaces serde-derive's confusing "can only be used on enums".
                //
                // The early return is intentional: this is the first match arm,
                // so `errors` is still empty here and nothing is dropped. The
                // downstream checks (visibility, generics, reserved idents) all
                // assume the event is an enum, so re-running them on a struct
                // would only add noise. Converting the struct to an enum is the
                // single required fix and re-surfaces any remaining diagnostics on
                // the next compile.
                return Err(errors.finish(SynError::new_spanned(
                    &item.ident,
                    ParseError::EventMustBeEnum,
                )));
            }
            StructOrEnumItem::Enum(item) => (&item.vis, &item.ident, &item.generics),
        };

        match vis {
            Visibility::Public(_) => {}
            Visibility::Inherited => {
                return Err(errors.finish(SynError::new_spanned(ident, ParseError::NoPrivateEvent)));
            }
            Visibility::Restricted(spec) => {
                return Err(
                    errors.finish(SynError::new_spanned(spec, ParseError::NoComplexVisibility))
                );
            }
        }

        if ident == &*idents::input() {
            errors.subsume(SynError::new_spanned(ident, ParseError::UseOfReservedIdent));
        }

        for generic in &generics.params {
            match generic {
                GenericParam::Lifetime(params) => {
                    if params.lifetime == *lifetimes::input() {
                        errors.subsume(SynError::new(
                            params.lifetime.span(),
                            ParseError::UseOfReservedLifetime,
                        ));
                    }
                }
                GenericParam::Type(params) => {
                    if params.ident == *idents::input() {
                        errors.subsume(SynError::new_spanned(
                            &params.ident,
                            ParseError::UseOfReservedIdent,
                        ));
                    }
                }
                GenericParam::Const(_) => {}
            }
        }

        errors.check()?;

        Ok(EventImpl {
            ident,
            generics,
            orig: input.item,
        })
    }
}

#[cfg(test)]
mod tests {
    use quote::ToTokens;
    use syn::parse_quote;

    use super::*;

    fn render(item: syn::ItemEnum) -> String {
        // Validation consults the reserved-ident table; initialize it first.
        crate::reserved::init();
        let item = StructOrEnumItem::Enum(item);
        EventImpl::try_from(EventImplInput { item: &item })
            .map_err(|_| "event impl should build for a valid enum")
            .unwrap()
            .to_token_stream()
            .to_string()
    }

    #[test]
    fn event_data_emits_empty_payload_for_unit_variants_and_drops_expect() {
        let rendered = render(parse_quote! {
            pub enum Event {
                Ping,
                Tick { count: u32 },
            }
        });

        // A unit variant must no longer serialize to the 4-byte JSON literal
        // `null`: `data()` returns an empty borrowed payload for a null/absent
        // `data` field.
        assert!(
            rendered.contains("Null"),
            "data() must special-case a null/absent `data` field, got:\n{rendered}",
        );
        assert!(
            rendered.contains("Borrowed"),
            "data() must return an empty borrowed payload for unit variants, got:\n{rendered}",
        );

        // The brittle `.expect(...)` that aborted the whole instance on a
        // malformed value is gone.
        assert!(
            !rendered.contains(". expect ("),
            "kind()/data() must not use `.expect`, got:\n{rendered}",
        );
    }
}
