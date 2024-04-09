use quote::{quote, ToTokens};

use super::arg;
use super::ty;
use crate::errors;

pub enum LogicMethod<'a> {
    Public(PublicLogicMethod<'a>),
    Private,
}

pub struct PublicLogicMethod<'a> {
    name: &'a syn::Ident,
    self_: &'a syn::Path,
    self_type: Option<arg::SelfType>,
    args: Vec<arg::LogicArgTyped<'a>>,
    ret: Option<ty::LogicTy>,
    lifetime: Option<syn::Lifetime>,
    // orig: &'a syn::ImplItemFn,
}

impl<'a> ToTokens for PublicLogicMethod<'a> {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        let self_ = &self.self_;
        let name = &self.name;
        let args = &self.args;

        let arg_idents = args.iter().map(|arg| &*arg.ident).collect::<Vec<_>>();

        let input = if args.is_empty()  {
            quote! {}
        } else {
            let lifetime = match &self.lifetime {
                Some(lifetime) => quote! { <#lifetime> },
                None => quote! {},
            };

            quote! {
                #[derive(::calimero_sdk::serde::Deserialize)]
                #[serde(crate = "::calimero_sdk::serde")]
                struct CalimeroInput #lifetime {
                    #(
                        #args
                    ),*
                }

                let Some(input) = ::calimero_sdk::env::input() else {
                    ::calimero_sdk::env::panic_str("Expected input since method has arguments.")
                };

                let CalimeroInput {
                    #(#arg_idents),*
                } = match ::calimero_sdk::serde_json::from_slice(&input) {
                    Ok(value) => value,
                    Err(err) => ::calimero_sdk::env::panic_str(
                        &format!("Failed to deserialize input from JSON: {:?}", err)
                    ),
                };
            }
        };

        let (def, call) = match &self.self_type {
            Some(type_) => {
                let def = match type_ {
                    arg::SelfType::Mutable =>quote! {
                        let mut app: #self_ = ::calimero_sdk::env::state_read().unwrap_or_default();
                    },
                    arg::SelfType::Immutable | arg::SelfType::Owned => quote! {
                        let Some(app) = ::calimero_sdk::env::state_read::<#self_>() else {
                            ::calimero_sdk::env::panic_str("Failed to read app state.")
                        };
                    },
                };

                (def, quote! { app.#name(#(#arg_idents),*); })
            }
            None => (quote! {}, quote! { #self_::#name(#(#arg_idents),*); }),
        };

        let call = match &self.ret {
            None => call,
            Some(_) => quote! {
                let output = #call;

                let output = {
                    #[allow(unused_imports)]
                    use ::calimero_sdk::__private::IntoResult;
                    match ::calimero_sdk::__private::WrappedReturn::new(output)
                        .into_result()
                        .to_json()
                    {
                        Ok(output) => output,
                        Err(err) => ::calimero_sdk::env::panic_str(
                            &format!("Failed to serialize output to JSON: {:?}", err)
                        ),
                    }
                };

                ::calimero_sdk::env::value_return(output);
            },
        };

        let state_finalizer = match &self.self_type {
            Some(arg::SelfType::Mutable) => quote! {
                ::calimero_sdk::env::state_write(&app);
            },
            _ => quote! {},
        };

        quote! {
            #[cfg(target_arch = "wasm32")]
            #[no_mangle]
            pub extern "C" fn #name() {
                ::calimero_sdk::env::setup_panic_hook();

                #input

                #def

                #call

                #state_finalizer
            }
        }
        .to_tokens(tokens)
    }
}

pub struct LogicMethodImplInput<'a> {
    pub type_: &'a syn::Path,
    pub item: &'a syn::ImplItemFn,
}

impl<'a> TryFrom<LogicMethodImplInput<'a>> for LogicMethod<'a> {
    type Error = errors::Errors<'a, syn::ImplItemFn>;

    fn try_from(input: LogicMethodImplInput<'a>) -> Result<Self, Self::Error> {
        if !matches!(input.item.vis, syn::Visibility::Public(_)) {
            return Ok(Self::Private);
        }

        let mut errors = errors::Errors::new(input.item);

        if let Some(asyncness) = &input.item.sig.asyncness {
            errors.push(asyncness, errors::ParseError::NoAsyncSupport);
        }

        if let Some(unsafety) = &input.item.sig.unsafety {
            errors.push(unsafety, errors::ParseError::NoUnsafeSupport);
        }

        for generic in &input.item.sig.generics.params {
            if let syn::GenericParam::Lifetime(_) = generic {
                // todo! ensure it's not <'calimero>
                continue;
            }
            errors.push(generic, errors::ParseError::NoGenericSupport);
        }

        let lifetime = syn::Lifetime::new("'CALIMERO_INPUT", proc_macro2::Span::call_site());

        let mut has_refs = false;
        let mut self_type = None;
        let mut args = vec![];
        for arg in &input.item.sig.inputs {
            match arg::LogicArg::try_from(arg::LogicArgInput {
                type_: input.type_,
                lifetime: &lifetime,
                arg,
            }) {
                Ok(arg) => match (arg, &self_type) {
                    (arg::LogicArg::Receiver(type_), None) => self_type = Some(type_),
                    (arg::LogicArg::Receiver(_), Some(_)) => { /* handled by rustc */ }
                    (arg::LogicArg::Typed(arg), _) => {
                        has_refs |= arg.ty.ref_;
                        args.push(arg)
                    }
                },
                Err(err) => errors = errors.subsume(err),
            }
        }

        let mut ret = None;
        if let syn::ReturnType::Type(_, ret_type) = &input.item.sig.output {
            match ty::LogicTy::try_from(ty::LogicTyInput {
                type_: input.type_,
                lifetime: &lifetime,
                ty: &*ret_type,
            }) {
                Ok(ty) => ret = Some(ty),
                Err(err) => errors = errors.subsume(err),
            }
        }

        errors.check(LogicMethod::Public(PublicLogicMethod {
            name: &input.item.sig.ident,
            self_: input.type_,
            self_type,
            args,
            lifetime: has_refs.then(|| lifetime),
            ret,
            // orig: item,
        }))
    }
}
