use proc_macro2::TokenStream;
use quote::{quote, ToTokens};
use syn::{Error as SynError, ImplItem, ItemImpl, Pat, PatType, Type};

use crate::errors::{Errors, ParseError};

pub struct CallbackImpl<'a> {
    ident: &'a syn::Ident,
    generics: &'a syn::Generics,
    methods: Vec<CallbackMethod<'a>>,
    orig: &'a ItemImpl,
}

pub struct CallbackMethod<'a> {
    ident: &'a syn::Ident,
    event_type: &'a Type,
}

impl ToTokens for CallbackMethod<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let ident = self.ident;
        let event_type = self.event_type;

        quote! {
            ::calimero_sdk::callback::register_callback_borrowed(
                "__all_events__", // Use universal callback key
                |event_value| {
                    // Deserialize the event from JSON
                    match ::calimero_sdk::serde_json::from_value::<#event_type>(event_value.clone()) {
                        Ok(event) => {
                            ::calimero_sdk::callback::with_current_app_mut(|app| {
                                app.#ident(event);
                            }).expect("Failed to get mutable app reference for callback");
                        }
                        Err(err) => {
                            ::calimero_sdk::env::log(&format!(
                                "Failed to deserialize event for callback {}: {:?}",
                                stringify!(#ident),
                                err
                            ));
                        }
                    }
                }
            );
        }
        .to_tokens(tokens);
    }
}

impl ToTokens for CallbackImpl<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let CallbackImpl {
            ident,
            generics,
            methods,
            orig,
        } = self;

        let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

        // Generate the original impl block with callback methods
        let items = &orig.items;
        let original_impl = quote! {
            impl #impl_generics #ident #ty_generics #where_clause {
                #(#items)*
            }
        };

        // Generate the callback registry
        let callback_registry = quote! {
            impl #impl_generics ::calimero_sdk::callback::CallbackRegistryTrait for #ident #ty_generics #where_clause {
                fn register_callbacks(&mut self) {
                    #(#methods)*
                }
            }
        };

        quote! {
            #original_impl
            #callback_registry
        }.to_tokens(tokens);
    }
}

pub struct CallbackImplInput<'a> {
    pub item: &'a ItemImpl,
}

impl<'a> TryFrom<CallbackImplInput<'a>> for CallbackImpl<'a> {
    type Error = Errors<'a, ItemImpl>;

    fn try_from(input: CallbackImplInput<'a>) -> Result<Self, Self::Error> {
        let errors = Errors::new(input.item);

        let mut methods = Vec::new();

        for item in &input.item.items {
            if let ImplItem::Fn(method) = item {
                // Extract event type from method signature
                if method.sig.inputs.len() != 2 {
                    errors.subsume(SynError::new_spanned(
                        &method.sig,
                        ParseError::CallbackMethodSignature,
                    ));
                    continue;
                }

                // First parameter should be &self, second should be the event
                let self_param = &method.sig.inputs[0];
                let event_param = &method.sig.inputs[1];

                let self_pat = match self_param {
                    syn::FnArg::Receiver(_) => true,
                    syn::FnArg::Typed(PatType { pat, .. }) => matches!(**pat, Pat::Ident(_)),
                };

                if !self_pat {
                    errors.subsume(SynError::new_spanned(
                        self_param,
                        ParseError::CallbackMethodSignature,
                    ));
                    continue;
                }

                let event_type = match event_param {
                    syn::FnArg::Typed(PatType { ty, .. }) => ty,
                    _ => {
                        errors.subsume(SynError::new_spanned(
                            event_param,
                            ParseError::CallbackMethodSignature,
                        ));
                        continue;
                    }
                };

                methods.push(CallbackMethod {
                    ident: &method.sig.ident,
                    event_type: &*event_type,
                });
            }
        }

        let ident = match &*input.item.self_ty {
            Type::Path(type_path) => match type_path.path.segments.last() {
                Some(seg) => &seg.ident,
                None => {
                    errors.subsume(SynError::new_spanned(
                        &input.item.self_ty,
                        ParseError::CallbackMethodSignature,
                    ));
                    return Err(errors.finish(SynError::new_spanned(
                        &input.item.self_ty,
                        ParseError::CallbackMethodSignature,
                    )));
                }
            },
            _ => {
                errors.subsume(SynError::new_spanned(
                    &input.item.self_ty,
                    ParseError::CallbackMethodSignature,
                ));
                return Err(errors.finish(SynError::new_spanned(
                    &input.item.self_ty,
                    ParseError::CallbackMethodSignature,
                )));
            }
        };

        errors.check()?;

        Ok(CallbackImpl {
            ident,
            generics: &input.item.generics,
            methods,
            orig: input.item,
        })
    }
}
