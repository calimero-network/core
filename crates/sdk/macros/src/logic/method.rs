use quote::{quote, ToTokens};

use super::{arg, ty};
use crate::{errors, reserved};

pub enum LogicMethod<'a> {
    Public(PublicLogicMethod<'a>),
    Private,
}

pub enum Modifer {
    Init(),
}

pub struct PublicLogicMethod<'a> {
    self_: syn::Path,

    name: &'a syn::Ident,
    self_type: Option<arg::SelfType>,
    args: Vec<arg::LogicArgTyped<'a>>,
    ret: Option<ty::LogicTy>,

    has_refs: bool,

    modifiers: Vec<Modifer>,
}

impl<'a> ToTokens for LogicMethod<'a> {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        match self {
            LogicMethod::Public(method) => method.to_tokens(tokens),
            LogicMethod::Private => {}
        }
    }
}

impl<'a> ToTokens for PublicLogicMethod<'a> {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        let self_ = &self.self_;
        let name = &self.name;
        let args = &self.args;
        let modifiers = &self.modifiers;
        let ret = &self.ret;

        let arg_idents = args.iter().map(|arg| &*arg.ident).collect::<Vec<_>>();

        let init_method = if modifiers.is_empty() {
            false
        } else {
            modifiers
                .iter()
                .any(|modifier| matches!(modifier, Modifer::Init()))
        };

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
            Some(_type_) => (
                quote! {
                    let Some(app) = ::calimero_sdk::env::state_read::<#self_>() else {
                        ::calimero_sdk::env::panic_str("Failed to read app state.")
                    };
                },
                quote! { app.#name(#(#arg_idents),*); },
            ),
            None => (
                match init_method {
                    true => quote! {let app: #self_ = },
                    false => quote! {},
                },
                quote! { <#self_>::#name(#(#arg_idents),*); },
            ),
        };

        if let Some(_) = &self.ret {
            //only when it's not init
            if !init_method {
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
        }

        let state_finalizer = {
            if init_method == true || matches!(&self.self_type, Some(arg::SelfType::Mutable)) {
                quote! {
                    ::calimero_sdk::env::state_write(&app);
                }
            } else {
                quote! {}
            }
        };

        let init_impl = {
            if init_method {
                quote! {
                    impl ::calimero_sdk::state::AppStateInit for #self_ {
                        type Return = #ret;
                    }
                }
            } else {
                quote! {}
            }
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
            #init_impl
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

        let mut modifiers = vec![];

        for attr in &input.item.attrs {
            if attr.path().segments.len() == 2
                && attr.path().segments[0].ident == "app"
                && attr.path().segments[1].ident == "init"
            {
                if let Some(_self_type) = &self_type {
                    errors.subsume(syn::Error::new_spanned(
                        input.item,
                        "The argument to #[app::init] method can't be self or mut self",
                    ));
                }
                modifiers.push(Modifer::Init())
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
            modifiers,
        }))
    }
}
