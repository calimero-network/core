use proc_macro2::TokenStream;
use quote::{quote, ToTokens};
use syn::{parse_quote, Error as SynError, GenericParam, Generics, Ident, Visibility, Type};

use crate::errors::{Errors, ParseError};
use crate::items::StructOrEnumItem;
use crate::reserved::{idents, lifetimes};
use crate::abi::register_event;
use calimero_wasm_abi_v1::{Event, TypeRef};

/// Dummy resolver for the normalizer
struct DummyResolver;

impl calimero_wasm_abi_v1::TypeResolver for DummyResolver {
    fn resolve_local(&self, _path: &str) -> Option<calimero_wasm_abi_v1::ResolvedLocal> {
        None
    }
}

/// Check if a type is Option<T>
fn is_option_type(ty: &Type) -> bool {
    if let Type::Path(path) = ty {
        if let Some(ident) = path.path.get_ident() {
            return ident.to_string() == "Option";
        }
    }
    false
}

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

        // Register the event in the global registry for ABI generation
        let event_name = ident.to_string();
        let abi_events = match orig {
            StructOrEnumItem::Struct(_) => {
                // For struct events, we'll create a simple event without payload for now
                // In a more sophisticated implementation, we'd analyze the struct fields
                vec![Event {
                    name: event_name,
                    payload: None,
                }]
            }
            StructOrEnumItem::Enum(item_enum) => {
                // For enum events, analyze each variant
                let mut events = Vec::new();
                for variant in &item_enum.variants {
                    let payload = match &variant.fields {
                        syn::Fields::Named(named_fields) => {
                            // For named fields, create a record type
                            let mut fields = Vec::new();
                            for field in &named_fields.named {
                                if let Some(ident) = &field.ident {
                                    let type_ref = calimero_wasm_abi_v1::normalize_type(&field.ty, true, &DummyResolver)
                                        .unwrap_or_else(|_| calimero_wasm_abi_v1::TypeRef::string());
                                    let nullable = is_option_type(&field.ty);
                                    
                                    fields.push(calimero_wasm_abi_v1::Field {
                                        name: ident.to_string(),
                                        type_: type_ref,
                                        nullable: if nullable { Some(true) } else { None },
                                    });
                                }
                            }
                            Some(calimero_wasm_abi_v1::TypeRef::Collection(
                                calimero_wasm_abi_v1::CollectionType::Record { fields }
                            ))
                        }
                        syn::Fields::Unnamed(fields) => {
                            if fields.unnamed.len() == 1 {
                                Some(calimero_wasm_abi_v1::normalize_type(&fields.unnamed[0].ty, true, &DummyResolver)
                                    .unwrap_or_else(|_| calimero_wasm_abi_v1::TypeRef::string()))
                            } else {
                                // Multiple unnamed fields - treat as generic
                                Some(calimero_wasm_abi_v1::TypeRef::string())
                            }
                        }
                        syn::Fields::Unit => None,
                    };
                    
                    events.push(Event {
                        name: variant.ident.to_string(),
                        payload,
                    });
                }
                events
            }
        };
        
        for event in abi_events {
            register_event(event);
        }

        quote! {
            #[derive(::calimero_sdk::serde::Serialize)]
            #[serde(crate = "::calimero_sdk::serde")]
            #[serde(tag = "kind", content = "data")]
            #orig

            impl #impl_generics ::calimero_sdk::event::AppEvent for #ident #ty_generics #where_clause {
                fn kind(&self) -> ::std::borrow::Cow<str> {
                    // todo! revisit quick
                    match ::calimero_sdk::serde_json::to_value(self) {
                        Ok(data) => ::std::borrow::Cow::Owned(data["kind"].as_str().expect("Failed to get event kind").to_string()),
                        Err(err) => ::calimero_sdk::env::panic_str(
                            &format!("Failed to serialize event: {:?}", err)
                        ),
                    }
                }
                fn data(&self) -> ::std::borrow::Cow<[u8]> {
                    // todo! revisit quick
                    match ::calimero_sdk::serde_json::to_value(self) {
                        Ok(data) => ::std::borrow::Cow::Owned(::calimero_sdk::serde_json::to_vec(&data["data"]).expect("Failed to serialize event data")),
                        Err(err) => ::calimero_sdk::env::panic_str(
                            &format!("Failed to serialize event: {:?}", err)
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
            StructOrEnumItem::Struct(item) => (&item.vis, &item.ident, &item.generics),
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
