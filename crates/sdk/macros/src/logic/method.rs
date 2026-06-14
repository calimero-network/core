use proc_macro2::{Span, TokenStream};
use quote::{quote, quote_spanned, ToTokens};
use syn::spanned::Spanned;
use syn::{
    Error as SynError, GenericArgument, GenericParam, Ident, ImplItemFn, Path, PathArguments,
    ReturnType, Type, Visibility,
};

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
    /// `#[app::view]` — app author declares the method read-only. Stored in
    /// the compiled ABI so the node can take a shared read lock instead of an
    /// exclusive write lock. Mutually exclusive with `Init` (an initializer
    /// always writes state).
    View,
    /// `#[app::xcall]` — app author declares the method a cross-context entry
    /// point. Stored in the compiled ABI (`Method.xcall_callable`) so the node
    /// can restrict `xcall` dispatch to declared entry points. Mutually
    /// exclusive with `Init` (an initializer is never an xcall target).
    XCall,
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
            // No declared arguments: a method that takes none must not silently
            // accept named ones. An absent body, an empty/`null` body, or any
            // non-object body carries no named arguments and is accepted (so
            // callers need not send `{}`); only a populated JSON object — i.e.
            // actual extra arguments — is rejected. Mirrors `deny_unknown_fields`
            // on the args-bearing branch below.
            quote_spanned! {name.span()=>
                if let Some(input) = ::calimero_sdk::env::input() {
                    if !input.is_empty() {
                        if let Ok(::calimero_sdk::serde_json::Value::Object(fields)) =
                            ::calimero_sdk::serde_json::from_slice::<
                                ::calimero_sdk::serde_json::Value,
                            >(&input)
                        {
                            if !fields.is_empty() {
                                ::calimero_sdk::env::panic_str(&format!(
                                    "Failed to deserialize input from JSON: method takes no \
                                     arguments but received unknown field(s): {:?}",
                                    fields.keys().collect::<::std::vec::Vec<_>>()
                                ));
                            }
                        }
                    }
                }
            }
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
                #[serde(crate = "::calimero_sdk::serde", deny_unknown_fields)]
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
            // Wrap init call to assign deterministic IDs after creation
            call = quote_spanned! {name.span()=>
                ::calimero_storage::collections::Root::new(|| {
                    let mut state = #call;
                    // Assign deterministic IDs to all collection fields based on field names
                    // This ensures CIP Invariant I9: all nodes generate identical entity IDs
                    state.__assign_deterministic_ids();
                    state
                })
            };

            quote_spanned! {name.span()=>
                #[cfg(target_arch = "wasm32")]
                #[no_mangle]
                pub extern "C" fn __calimero_sync_next() {
                    // Route panic location + message through the host so a
                    // WASM trap during apply surfaces the actual Rust
                    // panic in node logs instead of a bare "unreachable".
                    // Other exported methods install this hook already
                    // (see the regular-method branch below); sync-next
                    // didn't, which is why upstream sync failures were
                    // hard to diagnose.
                    ::calimero_sdk::env::setup_panic_hook();

                    let Some(args) = ::calimero_sdk::env::input() else {
                        ::calimero_sdk::env::panic_str("Expected payload to sync method.")
                    };

                    // #2266: ctx is empty here as a TEMPLATE — used as-is for
                    // `StorageDelta::Actions` (SDK-driven local apply, v2
                    // stored-writers fallback). For network-sync deltas the
                    // node sync layer ships a `StorageDelta::CausalActions`
                    // artifact carrying pre-resolved `effective_writers`;
                    // `Root::sync` branches on the variant and builds a
                    // per-action ctx from the map, ignoring this template.
                    let __sync_ctx = ::calimero_storage::interface::ApplyContext::empty();
                    ::calimero_storage::collections::Root::<#self_>::sync(&args, &__sync_ctx).expect("fatal: sync failed");
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
                ::calimero_sdk::env::init_logging();

                ::calimero_sdk::event::register::<#self_>();
                // PR-6c: surface this binary's SCHEMA_VERSION so the
                // type-erased `app::schema_version()` (read at the
                // identity-gated storage stamp site) reflects the active
                // target on a real node, mirroring the event-emitter register
                // above. Without this every entrypoint would leave it at the
                // unversioned 0, mis-stamping converted entries.
                ::calimero_sdk::app::register_schema_version::<#self_>();

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

/// Detects a bare `Result<T, String>` return type and returns the span of the
/// `String` error type, so app logic methods can be steered towards
/// `app::Result<T>` (which carries `app::Error`).
///
/// `app::Result<T>` is left alone: it desugars to `Result<T, app::Error>` but
/// is written with a single type argument, so it never matches the two-argument
/// `Result<Ok, Err>` shape this looks for. Custom error types
/// (`Result<T, MyError>`) are likewise untouched.
///
/// This is a purely syntactic check, so it cannot see through type aliases: a
/// `type MyResult<T> = Result<T, String>` used as `-> MyResult<T>` is not
/// flagged. The error type is matched only against the standard-library
/// `String` spellings (`String`, `std::string::String`, `alloc::string::String`)
/// so an unrelated user type whose last path segment happens to be `String`
/// is not falsely flagged.
fn string_error_result_span(ty: &Type) -> Option<Span> {
    let Type::Path(type_path) = ty else {
        return None;
    };

    let segment = type_path.path.segments.last()?;
    if segment.ident != "Result" {
        return None;
    }

    let PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };

    let mut type_args = args.args.iter().filter_map(|arg| match arg {
        GenericArgument::Type(ty) => Some(ty),
        _ => None,
    });

    let _ok = type_args.next()?;
    let err = type_args.next()?;
    if type_args.next().is_some() {
        // More than two type arguments — not a plain `Result<Ok, Err>`.
        return None;
    }

    is_std_string(err).then(|| err.span())
}

/// Whether `ty` is the standard-library `String`, written either bare or with a
/// `std`/`alloc` qualifier. A leading `::` is allowed; any other prefix (e.g.
/// `my_crate::String`) is rejected to avoid false positives.
fn is_std_string(ty: &Type) -> bool {
    let Type::Path(type_path) = ty else {
        return false;
    };

    if type_path.qself.is_some() {
        return false;
    }

    let segments = &type_path.path.segments;

    // The `String` segment itself must carry no generic arguments.
    if !segments
        .last()
        .is_some_and(|segment| segment.arguments.is_empty())
    {
        return false;
    }

    match segments.len() {
        1 => segments[0].ident == "String",
        3 => {
            (segments[0].ident == "std" || segments[0].ident == "alloc")
                && segments[1].ident == "string"
                && segments[2].ident == "String"
        }
        _ => false,
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
            if attr.path().segments.len() == 2 && attr.path().segments[0].ident == "app" {
                match attr.path().segments[1].ident.to_string().as_str() {
                    "init" => {
                        modifiers.push(Modifer::Init);
                        is_init = true;
                    }
                    "view" => {
                        modifiers.push(Modifer::View);
                    }
                    "xcall" => {
                        modifiers.push(Modifer::XCall);
                    }
                    _ => {}
                }
            }
        }

        let is_view = modifiers.iter().any(|m| matches!(m, Modifer::View));
        if is_init && is_view {
            errors.subsume(SynError::new_spanned(
                input.item,
                ParseError::ViewAndInitConflict,
            ));
        }

        let is_xcall = modifiers.iter().any(|m| matches!(m, Modifer::XCall));
        if is_init && is_xcall {
            errors.subsume(SynError::new_spanned(
                input.item,
                ParseError::XCallAndInitConflict,
            ));
        }
        if is_view && is_xcall {
            errors.subsume(SynError::new_spanned(
                input.item,
                ParseError::XCallAndViewConflict,
            ));
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
            if let Some(span) = string_error_result_span(ret_type) {
                errors.subsume(SynError::new(span, ParseError::StringErrorResult));
            }

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

#[cfg(test)]
mod tests {
    use syn::{parse_quote, Type};

    use super::string_error_result_span;

    fn flags(ty: Type) -> bool {
        string_error_result_span(&ty).is_some()
    }

    #[test]
    fn flags_bare_string_error_result() {
        assert!(flags(parse_quote! { Result<(), String> }));
        assert!(flags(parse_quote! { Result<u64, String> }));
        assert!(flags(parse_quote! { Result<Vec<FileRecord>, String> }));
        assert!(flags(parse_quote! { core::result::Result<(), String> }));
        assert!(flags(parse_quote! { std::result::Result<bool, String> }));
        // Qualified standard-library `String` spellings.
        assert!(flags(parse_quote! { Result<(), std::string::String> }));
        assert!(flags(parse_quote! { Result<(), alloc::string::String> }));
    }

    #[test]
    fn ignores_app_result_and_custom_errors() {
        // `app::Result<T>` carries a single type argument.
        assert!(!flags(parse_quote! { app::Result<u64> }));
        assert!(!flags(parse_quote! { Result<u64> }));
        // Custom / non-`String` error types are fine.
        assert!(!flags(parse_quote! { Result<u64, MyError> }));
        assert!(!flags(parse_quote! { Result<u64, std::io::Error> }));
        assert!(!flags(parse_quote! { Result<u64, app::Error> }));
        // A user type whose last segment is `String` must not be flagged.
        assert!(!flags(parse_quote! { Result<u64, my_crate::String> }));
        assert!(!flags(
            parse_quote! { Result<u64, widgets::string::String> }
        ));
    }

    #[test]
    fn ignores_non_result_types() {
        assert!(!flags(parse_quote! { u64 }));
        assert!(!flags(parse_quote! { Option<String> }));
        assert!(!flags(parse_quote! { String }));
        // More than two type arguments is not a plain `Result<Ok, Err>`.
        assert!(!flags(parse_quote! { Result<u64, String, Extra> }));
    }
}
