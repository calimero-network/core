use proc_macro2::TokenStream;
use quote::{quote, ToTokens};
use syn::{Error as SynError, ItemImpl, Pat, PatType, Type, ImplItem};

use crate::errors::{Errors, ParseError};

pub struct CallbackImpl<'a> {
    ident: &'a syn::Ident,
    generics: &'a syn::Generics,
    methods: Vec<CallbackMethod<'a>>,
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
            ::calimero_sdk::callback::register_callback::<#event_type, _>(
                stringify!(#ident),
                |event| {
                    self.#ident(event);
                }
            );
        }.to_tokens(tokens);
    }
}

impl ToTokens for CallbackImpl<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let CallbackImpl {
            ident,
            generics,
            methods,
        } = self;

        let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

        // Generate the callback registry
        let callback_registry = quote! {
            impl #impl_generics ::calimero_sdk::callback::CallbackRegistry for #ident #ty_generics #where_clause {
                fn register_callbacks(&self) {
                    #(#methods)*
                }
            }
        };

        callback_registry.to_tokens(tokens);
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
            Type::Path(type_path) => &type_path.path.segments.last().unwrap().ident,
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
        })
    }
}
