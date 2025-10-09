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
    Callback(CallbackMethod<'a>),
    Private,
}

pub enum Modifer {
    Init,
}

pub struct CallbackMethod<'a> {
    name: &'a Ident,
    event_type: syn::Type,
}

impl ToTokens for CallbackMethod<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let name = &self.name;
        let event_type = &self.event_type;

        quote! {
            // Register a universal callback that handles all event types
            // The callback method will receive the full Event enum and match internally
            ::calimero_sdk::callback::register_callback_borrowed(
                "__all_events__", // Use a special key for universal callbacks
                |event_value| {
                    // Deserialize the event from JSON with the appropriate lifetime
                    match ::calimero_sdk::serde_json::from_value::<#event_type>(event_value.clone()) {
                        Ok(event) => {
                            ::calimero_sdk::callback::with_current_app_mut(|app| {
                                app.#name(event);
                            }).expect("Failed to get mutable app reference for callback");
                        }
                        Err(err) => {
                            ::calimero_sdk::env::log(&format!(
                                "Failed to deserialize event for callback {}: {:?}",
                                stringify!(#name),
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
            LogicMethod::Callback(method) => method.to_tokens(tokens),
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
        let mut is_callback = false;

        for attr in &input.item.attrs {
            if attr.path().segments.len() == 2
                && attr.path().segments[0].ident == "app"
            {
                match attr.path().segments[1].ident.to_string().as_str() {
                    "init" => {
                        modifiers.push(Modifer::Init);
                        is_init = true;
                    }
                    "callback" => {
                        is_callback = true;
                    }
                    _ => {}
                }
            }
        }

        if is_callback {
            // Callback methods must be public and have exactly 2 parameters (&mut self, event)
            if !matches!(input.item.vis, Visibility::Public(_)) {
                errors.subsume(SynError::new_spanned(
                    &input.item.vis,
                    ParseError::NoPrivateInit, // Reuse this error for now
                ));
            }
            
            if input.item.sig.inputs.len() != 2 {
                errors.subsume(SynError::new_spanned(
                    &input.item.sig,
                    ParseError::CallbackMethodSignature,
                ));
            }
            
            // Validate that the second parameter is Event (with or without lifetime)
            let event_type = if input.item.sig.inputs.len() >= 2 {
                if let syn::FnArg::Typed(pat_type) = &input.item.sig.inputs[1] {
                    // Check if it's Event type
                    let is_valid_event_type = if let syn::Type::Path(type_path) = pat_type.ty.as_ref() {
                        // Check if it's Event (with or without lifetime/generic parameters)
                        type_path.path.segments.last().map_or(false, |seg| seg.ident == "Event")
                    } else {
                        false
                    };
                    
                    if !is_valid_event_type {
                        errors.subsume(SynError::new_spanned(
                            &pat_type.ty,
                            ParseError::CallbackMethodSignature,
                        ));
                    }
                    
                    pat_type.ty.clone()
                } else {
                    return Err(errors.finish(SynError::new_spanned(
                        &input.item.sig.inputs[1],
                        ParseError::CallbackMethodSignature,
                    )));
                }
            } else {
                return Err(errors.finish(SynError::new_spanned(
                    &input.item.sig,
                    ParseError::CallbackMethodSignature,
                )));
            };
            
            errors.check()?;
            
            return Ok(Self::Callback(CallbackMethod {
                name: &input.item.sig.ident,
                event_type: *event_type,
            }));
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
