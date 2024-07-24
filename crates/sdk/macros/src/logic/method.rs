use quote::{quote, ToTokens};

use super::{arg, ty};
use crate::{errors, reserved};

pub enum LogicMethod<'a> {
    Public(PublicLogicMethod<'a>),
    Private,
    Init(InitMethod<'a>),
}

pub struct PublicLogicMethod<'a> {
    self_: syn::Path,

    name: &'a syn::Ident,
    self_type: Option<arg::SelfType>,
    args: Vec<arg::LogicArgTyped<'a>>,
    ret: Option<ty::LogicTy>,

    has_refs: bool,
}

pub struct InitMethod<'a> {
    self_: syn::Path,
    name: &'a syn::Ident,
    args: Vec<arg::LogicArgTyped<'a>>,
}

impl<'a> ToTokens for LogicMethod<'a> {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        match self {
            LogicMethod::Public(method) => method.to_tokens(tokens),
            LogicMethod::Private => {}
            LogicMethod::Init(method) => method.to_tokens(tokens),
        }
    }
}

impl<'a> ToTokens for InitMethod<'a> {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        let self_ = &self.self_;
        let name = &self.name;
        let args = &self.args;

        let arg_idents = args.iter().map(|arg| &*arg.ident).collect::<Vec<_>>();
        let arg_types = args.iter().map(|arg| &arg.ty).collect::<Vec<_>>();

        quote! {
            #[cfg(target_arch = "wasm32")]
            #[no_mangle]
            pub extern "C" fn init() {
                ::calimero_sdk::env::setup_panic_hook();

                ::calimero_sdk::event::register::<#self_>();

                let input = ::calimero_sdk::env::input().expect("Init parameters are required");
                let (#(#arg_idents),*): (#(#arg_types),*) = ::calimero_sdk::serde_json::from_slice(&input)
                    .expect("Failed to deserialize init parameters");

                let state: #self_ = #self_::#name(#(#arg_idents),*);
                ::calimero_sdk::env::state_write(&state);
            }
        }
        .to_tokens(tokens)
    }
}

impl<'a> ToTokens for PublicLogicMethod<'a> {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        let self_ = &self.self_;
        let name = &self.name;
        let args = &self.args;

        let arg_idents = args.iter().map(|arg| &*arg.ident).collect::<Vec<_>>();

        let input = if args.is_empty() {
            quote! {}
        } else {
            let input_ident = reserved::idents::input();

            let input_lifetime = if self.has_refs {
                let lifetime = reserved::lifetimes::input();
                quote! { <#lifetime> }
            } else {
                quote! {}
            };

            quote! {
                #[derive(::calimero_sdk::serde::Deserialize)]
                #[serde(crate = "::calimero_sdk::serde")]
                struct #input_ident #input_lifetime {
                    #(
                        #args
                    ),*
                }

                let Some(input) = ::calimero_sdk::env::input() else {
                    ::calimero_sdk::env::panic_str("Expected input since method has arguments.")
                };

                let #input_ident {
                    #(#arg_idents),*
                } = match ::calimero_sdk::serde_json::from_slice(&input) {
                    Ok(value) => value,
                    Err(err) => ::calimero_sdk::env::panic_str(
                        &format!("Failed to deserialize input from JSON: {:?}", err)
                    ),
                };
            }
        };

        let (def, mut call) = match &self.self_type {
            Some(type_) => {
                let def = match type_ {
                    arg::SelfType::Mutable => quote! {
                        let mut app: #self_ = match ::calimero_sdk::env::state_read(){
                            Some(app) => app,
                            None => ::calimero_sdk::env::panic_str(&format!(
                                "Failed to read app state. App state was not initialized"
                            )),
                        };
                    },
                    arg::SelfType::Immutable | arg::SelfType::Owned => quote! {
                        let Some(app) = ::calimero_sdk::env::state_read::<#self_>() else {
                            ::calimero_sdk::env::panic_str("Failed to read app state.")
                        };
                    },
                };

                (def, quote! { app.#name(#(#arg_idents),*); })
            }
            None => (quote! {}, quote! { <#self_>::#name(#(#arg_idents),*); }),
        };

        if let Some(_) = &self.ret {
            call = quote! {
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
            };
        }

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

                ::calimero_sdk::event::register::<#self_>();

                #input

                #def

                #call

                #state_finalizer
            }
        }
        .to_tokens(tokens)
    }
}

pub struct LogicMethodImplInput<'a, 'b> {
    pub item: &'a syn::ImplItemFn,

    pub type_: &'b syn::Path,
}

impl<'a, 'b> TryFrom<LogicMethodImplInput<'a, 'b>> for LogicMethod<'a> {
    type Error = errors::Errors<'a, syn::ImplItemFn>;

    fn try_from(input: LogicMethodImplInput<'a, 'b>) -> Result<Self, Self::Error> {
        if !matches!(input.item.vis, syn::Visibility::Public(_)) {
            return Ok(Self::Private);
        }

        let mut errors = errors::Errors::new(input.item);

        // Check if this is an init method
        if input.item.attrs.iter().any(|attr| {
            attr.path().segments.len() == 2
                && attr.path().segments[0].ident == "app"
                && attr.path().segments[1].ident == "init"
        }) {
            // Process init method
            let mut args = vec![];
            for arg in &input.item.sig.inputs {
                match arg::LogicArg::try_from(arg::LogicArgInput {
                    type_: input.type_,
                    arg,
                }) {
                    Ok(arg::LogicArg::Typed(arg)) => args.push(arg),
                    Ok(arg::LogicArg::Receiver(_)) => {
                        errors.subsume(syn::Error::new_spanned(
                            arg,
                            "Init method should not have a receiver",
                        ));
                    }
                    Err(err) => errors.combine(err),
                }
            }

            errors.check()?;

            return Ok(LogicMethod::Init(InitMethod {
                self_: input.type_.clone(),
                name: &input.item.sig.ident,
                args,
            }));
        }

        if let Some(abi) = &input.item.sig.abi {
            errors.subsume(syn::Error::new_spanned(
                abi,
                errors::ParseError::NoExplicitAbi,
            ));
        }

        if let Some(asyncness) = &input.item.sig.asyncness {
            errors.subsume(syn::Error::new_spanned(
                asyncness,
                errors::ParseError::NoAsyncSupport,
            ));
        }

        if let Some(unsafety) = &input.item.sig.unsafety {
            errors.subsume(syn::Error::new_spanned(
                unsafety,
                errors::ParseError::NoUnsafeSupport,
            ));
        }

        for generic in &input.item.sig.generics.params {
            if let syn::GenericParam::Lifetime(params) = generic {
                if params.lifetime == *reserved::lifetimes::input() {
                    errors.subsume(syn::Error::new(
                        params.lifetime.span(),
                        errors::ParseError::UseOfReservedLifetime,
                    ));
                }
                continue;
            }
            errors.subsume(syn::Error::new_spanned(
                generic,
                errors::ParseError::NoGenericTypeSupport,
            ));
        }

        let mut has_refs = false;
        let mut self_type = None;
        let mut args = vec![];
        for arg in &input.item.sig.inputs {
            match arg::LogicArg::try_from(arg::LogicArgInput {
                type_: input.type_,
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
                Err(err) => errors.combine(err),
            }
        }

        let mut ret = None;
        if let syn::ReturnType::Type(_, ret_type) = &input.item.sig.output {
            match ty::LogicTy::try_from(ty::LogicTyInput {
                type_: input.type_,
                ty: &*ret_type,
            }) {
                Ok(ty) => ret = Some(ty),
                Err(err) => errors.combine(err),
            }
        }

        errors.check()?;

        Ok(LogicMethod::Public(PublicLogicMethod {
            name: &input.item.sig.ident,
            self_: input.type_.clone(),
            self_type,
            args,
            ret,
            has_refs,
        }))
    }
}
