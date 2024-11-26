use proc_macro2::TokenStream;
use quote::{quote, quote_spanned, ToTokens};
use syn::spanned::Spanned;
use syn::{Error as SynError, GenericParam, Ident, ImplItemFn, Path, ReturnType, Visibility};

use crate::errors::{Errors, ParseError};
use crate::logic::arg::{LogicArg, LogicArgInput, LogicArgTyped, SelfType};
use crate::logic::ty::{LogicTy, LogicTyInput};
use crate::reserved::{idents, lifetimes};

pub enum LogicMethod<'a> {
    Public(PublicLogicMethod<'a>),
    Private,
}

pub enum Modifer {
    Init,
}

pub struct PublicLogicMethod<'a> {
    self_: Path,

    name: &'a Ident,
    self_type: Option<SelfType<'a>>,
    args: Vec<LogicArgTyped<'a>>,
    ret: Option<LogicTy>,

    has_refs: bool,

    modifiers: Vec<Modifer>,
}

impl ToTokens for LogicMethod<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        match self {
            LogicMethod::Public(method) => method.to_tokens(tokens),
            LogicMethod::Private => {}
        }
    }
}

impl ToTokens for PublicLogicMethod<'_> {
    // TODO: Consider splitting this long function into multiple parts.
    #[expect(clippy::too_many_lines, reason = "TODO: This needs refactoring")]
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let self_ = &self.self_;
        let name = &self.name;
        let args = &self.args;
        let modifiers = &self.modifiers;
        let ret = &self.ret;

        let arg_idents = args.iter().map(|arg| arg.ident).collect::<Vec<_>>();

        let init_method = modifiers
            .iter()
            .any(|modifier| matches!(modifier, Modifer::Init));

        let input = if args.is_empty() {
            quote! {}
        } else {
            let input_ident = idents::input();

            let input_lifetime = if self.has_refs {
                let lifetime = lifetimes::input();
                quote_spanned! { name.span()=>
                    <#lifetime>
                }
            } else {
                quote! {}
            };

            quote_spanned! {name.span()=>
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

        #[expect(clippy::option_if_let_else, reason = "This is clearer this way")]
        let (def, mut call) = match &self.self_type {
            Some(type_) => (
                {
                    let (mutability, ty) = match type_ {
                        SelfType::Mutable(ty) => (Some(quote! {mut}), ty),
                        SelfType::Owned(ty) | SelfType::Immutable(ty) => (None, ty),
                    };
                    quote_spanned! {ty.span()=>
                        let Some(#mutability app) = ::calimero_storage::collections::Root::<#self_>::fetch()
                        else {
                            ::calimero_sdk::env::panic_str("Failed to find or read app state")
                        };
                    }
                },
                quote_spanned! {name.span()=>
                    app.#name(#(#arg_idents),*)
                },
            ),
            None => (
                if init_method {
                    quote_spanned! {name.span()=>
                        if ::calimero_storage::collections::Root::<#self_>::fetch().is_some() {
                            ::calimero_sdk::env::panic_str("Cannot initialize over already existing state.")
                        };

                        let app =
                    }
                } else {
                    quote! {}
                },
                quote_spanned! {name.span()=>
                    <#self_>::#name(#(#arg_idents),*)
                },
            ),
        };

        if let (Some(ret), false) = (&self.ret, init_method) {
            call = quote_spanned! {ret.ty.span()=>
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
                ::calimero_sdk::env::value_return(&output)
            }
        }

        let state_finalizer = match (&self.self_type, init_method) {
            (Some(SelfType::Mutable(_)), _) | (_, true) => quote! {
                app.commit();
            },
            _ => quote! {},
        };

        // todo! when generics are present, strip them
        let init_impl = if init_method {
            call = quote_spanned! {name.span()=>
                ::calimero_storage::collections::Root::new(|| #call)
            };

            quote_spanned! {name.span()=>
                #[cfg(target_arch = "wasm32")]
                #[no_mangle]
                pub extern "C" fn __calimero_sync_next() {
                    let Some(args) = ::calimero_sdk::env::input() else {
                        ::calimero_sdk::env::panic_str("Expected payload to sync method.")
                    };

                    ::calimero_storage::collections::Root::<#self_>::sync(&args).expect("fatal: sync failed");
                }

                impl ::calimero_sdk::state::AppStateInit for #self_ {
                    type Return = #ret;
                }
            }
        } else {
            quote! {}
        };

        quote_spanned! {name.span()=>
            #[cfg(target_arch = "wasm32")]
            #[no_mangle]
            pub extern "C" fn #name() {
                ::calimero_sdk::env::setup_panic_hook();

                ::calimero_sdk::event::register::<#self_>();

                #input

                #def

                #call;

                #state_finalizer
            }

            #init_impl

        }
        .to_tokens(tokens);
    }
}

pub struct LogicMethodImplInput<'a, 'b> {
    pub item: &'a ImplItemFn,

    pub type_: &'b Path,
}

impl<'a, 'b> TryFrom<LogicMethodImplInput<'a, 'b>> for LogicMethod<'a> {
    type Error = Errors<'a, ImplItemFn>;

    // TODO: Consider splitting this long function into multiple parts.
    #[expect(clippy::too_many_lines, reason = "TODO: This needs refactoring")]
    fn try_from(input: LogicMethodImplInput<'a, 'b>) -> Result<Self, Self::Error> {
        let errors = Errors::new(input.item);

        let mut modifiers = vec![];
        let mut is_init = false;

        for attr in &input.item.attrs {
            if attr.path().segments.len() == 2
                && attr.path().segments[0].ident == "app"
                && attr.path().segments[1].ident == "init"
            {
                modifiers.push(Modifer::Init);
                is_init = true;
            }
        }

        match (&input.item.vis, is_init) {
            (Visibility::Public(_), _) => {}
            (_, true) => {
                errors.subsume(SynError::new_spanned(
                    &input.item.vis,
                    ParseError::NoPrivateInit,
                ));
            }
            (_, false) => {
                return Ok(Self::Private);
            }
        }

        if let Some(abi) = &input.item.sig.abi {
            errors.subsume(SynError::new_spanned(abi, ParseError::NoExplicitAbi));
        }

        if let Some(asyncness) = &input.item.sig.asyncness {
            errors.subsume(SynError::new_spanned(asyncness, ParseError::NoAsyncSupport));
        }

        if let Some(unsafety) = &input.item.sig.unsafety {
            errors.subsume(SynError::new_spanned(unsafety, ParseError::NoUnsafeSupport));
        }

        for generic in &input.item.sig.generics.params {
            if let GenericParam::Lifetime(params) = generic {
                if params.lifetime == *lifetimes::input() {
                    errors.subsume(SynError::new(
                        params.lifetime.span(),
                        ParseError::UseOfReservedLifetime,
                    ));
                }
                continue;
            }
            errors.subsume(SynError::new_spanned(
                generic,
                ParseError::NoGenericTypeSupport,
            ));
        }

        let mut has_refs = false;
        let mut self_type = None;
        let mut args = vec![];
        for arg in &input.item.sig.inputs {
            match LogicArg::try_from(LogicArgInput {
                type_: input.type_,
                arg,
            }) {
                Ok(arg) => match (arg, &self_type) {
                    (LogicArg::Receiver(type_), None) => self_type = Some(type_),
                    (LogicArg::Receiver(_), Some(_)) => { /* handled by rustc */ }
                    (LogicArg::Typed(arg), _) => {
                        has_refs |= arg.ty.ref_;
                        args.push(arg);
                    }
                },
                Err(err) => errors.combine(&err),
            }
        }

        let name = &input.item.sig.ident;

        match (is_init, &self_type) {
            (true, Some(self_type)) => errors.subsume(SynError::new_spanned(
                match self_type {
                    SelfType::Owned(ty) | SelfType::Mutable(ty) | SelfType::Immutable(ty) => ty,
                },
                ParseError::NoSelfReceiverAtInit,
            )),
            (true, None) if name != "init" => errors.subsume(SynError::new_spanned(
                name,
                ParseError::AppInitMethodNotNamedInit,
            )),
            (false, _) if name == "init" => errors.subsume(SynError::new_spanned(
                name,
                ParseError::InitMethodWithoutInitAttribute,
            )),
            _ => {}
        }

        let mut ret = None;
        if let ReturnType::Type(_, ret_type) = &input.item.sig.output {
            match LogicTy::try_from(LogicTyInput {
                type_: input.type_,
                ty: ret_type,
            }) {
                Ok(ty) => ret = Some(ty),
                Err(err) => errors.combine(&err),
            }
        }

        errors.check()?;

        Ok(LogicMethod::Public(PublicLogicMethod {
            name,
            self_: input.type_.clone(),
            self_type,
            args,
            ret,
            has_refs,
            modifiers,
        }))
    }
}
