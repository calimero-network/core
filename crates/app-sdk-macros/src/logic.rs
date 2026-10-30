//! `#[app::logic]`: the methods block. It strips the method attributes, implements the
//! SDK's `Dispatch` for the state type (argument decoding, the call, result encoding), and
//! emits the `handle` export and the ABI in the `calimero:abi` custom section.
//!
//! Exposed methods are the `pub` ones plus the `#[app::init]` one; other functions stay
//! private helpers. ABI names are the Rust names in camelCase.

use proc_macro2::{Span, TokenStream};
use quote::{quote, ToTokens};
use serde_json::{json, Map, Value};
use syn::{
    Attribute, FnArg, ImplItem, ImplItemFn, ItemImpl, LitStr, Pat, ReceiverKind, ReturnType, Type,
    Visibility,
};

use crate::types::{
    abi_type, camel, canonical, check_wire, generic_arg, is_unit, valid_rate, wire, Wire,
};

/// The world this SDK builds for.
const WORLD: &str = "calimero:app@0.5.1";
/// The ABI version this SDK writes (D51).
const ABI_VERSION: u64 = 1;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Init,
    Call,
    View,
    Timer,
}

#[derive(Default)]
struct Attrs {
    init: bool,
    view: bool,
    confirm: bool,
    public: bool,
    rate: Option<String>,
    every: Option<String>,
    allow_system: bool,
    allow_apps: Vec<String>,
}

struct Method {
    rust: syn::Ident,
    abi: String,
    kind: Kind,
    attrs: Attrs,
    receiver_mut: Option<bool>,
    params: Vec<(syn::Ident, Type)>,
    /// The success type; `None` when the method returns `Result<T>`-less `()`.
    ok: Type,
    returns_result: bool,
}

/// The `app::<name>` attribute name, if `attr` is one (`app::x` or `calimero_app_sdk::app::x`).
fn app_attr(attr: &Attribute) -> Option<String> {
    let segs: Vec<String> = attr
        .path()
        .segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect();
    match segs.as_slice() {
        [app, name] if app == "app" => Some(name.clone()),
        [sdk, app, name] if sdk == "calimero_app_sdk" && app == "app" => Some(name.clone()),
        _ => None,
    }
}

/// `"<n>s"`, `"<n>m"` or `"<n>h"` with `n > 0`, as the host reads it.
fn valid_every(text: &str) -> bool {
    let Some(unit) = text.chars().last() else {
        return false;
    };
    matches!(unit, 's' | 'm' | 'h') && text[..text.len() - 1].parse::<u64>().is_ok_and(|n| n > 0)
}

fn parse_attrs(f: &mut ImplItemFn) -> syn::Result<Attrs> {
    let mut attrs = Attrs::default();
    let mut kept = Vec::new();
    for attr in f.attrs.drain(..) {
        let Some(name) = app_attr(&attr) else {
            kept.push(attr);
            continue;
        };
        match name.as_str() {
            "init" => attrs.init = true,
            "view" => attrs.view = true,
            "confirm" => attrs.confirm = true,
            "public" => {
                attrs.public = true;
                if matches!(attr.meta, syn::Meta::List(_)) {
                    attr.parse_nested_meta(|meta| {
                        if meta.path.is_ident("rate") {
                            let lit: LitStr = meta.value()?.parse()?;
                            if !valid_rate(&lit.value()) {
                                return Err(syn::Error::new(
                                    lit.span(),
                                    "a rate is `<n>/s`, `<n>/min` or `<n>/h` with n > 0",
                                ));
                            }
                            attrs.rate = Some(lit.value());
                            Ok(())
                        } else {
                            Err(meta.error("expected `rate = \"<n>/<unit>\"`"))
                        }
                    })?;
                }
            }
            "every" => {
                let lit: LitStr = attr.parse_args()?;
                if !valid_every(&lit.value()) {
                    return Err(syn::Error::new(
                        lit.span(),
                        "an interval is `<n>s`, `<n>m` or `<n>h` with n > 0",
                    ));
                }
                attrs.every = Some(lit.value());
                // A timer's caller is always `System` (D13), so `every` implies it (Ruling 26).
                attrs.allow_system = true;
            }
            "allow" => attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("system") {
                    attrs.allow_system = true;
                    Ok(())
                } else if meta.path.is_ident("app") {
                    let lit: LitStr = meta.value()?.parse()?;
                    attrs.allow_apps.push(lit.value());
                    Ok(())
                } else {
                    Err(meta.error("expected `system` or `app = \"<app id>\"`"))
                }
            })?,
            other => {
                return Err(syn::Error::new_spanned(
                    &attr,
                    format!("`app::{other}` is not a method attribute in this SDK version"),
                ))
            }
        }
    }
    f.attrs = kept;
    Ok(attrs)
}

fn method(f: &mut ImplItemFn) -> syn::Result<Option<Method>> {
    let attrs = parse_attrs(f)?;
    let public = matches!(f.vis, Visibility::Public(_));
    if !public && !attrs.init {
        let any = attrs.view || attrs.confirm || attrs.public || attrs.every.is_some();
        if any || attrs.allow_system || !attrs.allow_apps.is_empty() {
            return Err(syn::Error::new_spanned(
                &f.sig.ident,
                "an app method must be `pub`",
            ));
        }
        return Ok(None);
    }
    let sig = &f.sig;
    let fail = |msg: &str| Err(syn::Error::new_spanned(&sig.ident, msg));
    if sig.asyncness.is_some() || !sig.generics.params.is_empty() {
        return fail("app methods are synchronous and take no generics");
    }

    let mut receiver_mut = None;
    let mut params = Vec::new();
    for arg in &sig.inputs {
        match arg {
            FnArg::Receiver(r) => match &r.kind {
                ReceiverKind::Reference(_, _, m) => receiver_mut = Some(m.is_some()),
                _ => return fail("app methods take `&self` or `&mut self`"),
            },
            FnArg::Typed(pt) => {
                let Pat::Ident(pi) = &*pt.pat else {
                    return Err(syn::Error::new_spanned(
                        &pt.pat,
                        "app method parameters are plain names",
                    ));
                };
                if matches!(&*pt.ty, Type::Reference(_)) {
                    return Err(syn::Error::new_spanned(
                        &pt.ty,
                        "app method parameters are owned values, decoded from JSON",
                    ));
                }
                check_wire(&pt.ty)?;
                params.push((pi.ident.clone(), (*pt.ty).clone()));
            }
        }
    }

    let kind = match (attrs.init, attrs.view, attrs.every.is_some()) {
        (true, false, false) => Kind::Init,
        (false, true, false) => Kind::View,
        (false, false, true) => Kind::Timer,
        (false, false, false) => Kind::Call,
        _ => return fail("`init`, `view` and `every` exclude each other"),
    };
    match kind {
        Kind::Init if receiver_mut.is_some() => return fail("`init` takes no `self`"),
        Kind::Init
            if attrs.confirm
                || attrs.public
                || attrs.allow_system
                || !attrs.allow_apps.is_empty() =>
        {
            return fail("`init` runs once as the owner; it takes no other attribute")
        }
        Kind::View if receiver_mut != Some(false) => return fail("a view takes `&self`"),
        Kind::View if attrs.confirm => return fail("a `confirm` method never runs as a view"),
        Kind::Timer if receiver_mut.is_none() => return fail("a timer takes `&mut self`"),
        Kind::Timer if !params.is_empty() => return fail("a timer takes no arguments"),
        Kind::Call if receiver_mut.is_none() => {
            return fail("an app method takes `&self` or `&mut self`")
        }
        _ => {}
    }

    let (ok, returns_result) = match &sig.output {
        ReturnType::Default => (syn::parse_quote!(()), false),
        ReturnType::Type(_, ty) => match generic_arg(ty, "Result") {
            Some(ok) => (ok.clone(), true),
            None => ((**ty).clone(), false),
        },
    };

    if kind != Kind::Init {
        check_wire(&ok)?;
    }

    Ok(Some(Method {
        abi: camel(&sig.ident.to_string()),
        rust: sig.ident.clone(),
        kind,
        attrs,
        receiver_mut,
        params,
        ok,
        returns_result,
    }))
}

fn abi_entry(m: &Method) -> Value {
    let kind = match m.kind {
        Kind::Init => "init",
        Kind::Call => "call",
        Kind::View => "view",
        Kind::Timer => "timer",
    };
    let params: Vec<Value> = m
        .params
        .iter()
        .map(|(name, ty)| json!([camel(&name.to_string()), abi_type(ty)]))
        .collect();
    let returns = if m.kind == Kind::Init || is_unit(&m.ok) {
        Value::Null
    } else {
        Value::String(abi_type(&m.ok))
    };
    let mut entry = Map::new();
    let _ = entry.insert("kind".into(), kind.into());
    let _ = entry.insert("params".into(), params.into());
    let _ = entry.insert("returns".into(), returns);
    let _ = entry.insert("confirm".into(), m.attrs.confirm.into());
    let _ = entry.insert("public".into(), m.attrs.public.into());
    let _ = entry.insert("allowSystem".into(), m.attrs.allow_system.into());
    if let Some(every) = &m.attrs.every {
        let _ = entry.insert("every".into(), every.clone().into());
    }
    if let Some(rate) = &m.attrs.rate {
        let _ = entry.insert("rate".into(), rate.clone().into());
    }
    if !m.attrs.allow_apps.is_empty() {
        let _ = entry.insert("allowApps".into(), m.attrs.allow_apps.clone().into());
    }
    Value::Object(entry)
}

fn run_arm(index: usize, m: &Method, state: &Type) -> TokenStream {
    let fields = m.params.iter().map(|(name, ty)| {
        let rename = camel(&name.to_string());
        match wire(ty) {
            Some(int) if int.is_option() => {
                let with = int.json_module();
                quote! { #[serde(rename = #rename, default, with = #with)] #name: #ty }
            }
            Some(int) => {
                let with = int.json_module();
                quote! { #[serde(rename = #rename, with = #with)] #name: #ty }
            }
            None => quote! { #[serde(rename = #rename)] #name: #ty },
        }
    });
    let names: Vec<_> = m.params.iter().map(|(name, _)| name).collect();
    let rust = &m.rust;
    let call = match m.receiver_mut {
        None => quote! { <#state>::#rust(#(#names),*) },
        Some(_) => quote! {
            {
                #[allow(unused_mut)]
                let mut __state = <#state as ::calimero_app_sdk::app::AppState>::load();
                __state.#rust(#(#names),*)
            }
        },
    };
    let call = if m.returns_result {
        quote! { #call.map_err(::core::convert::Into::<::calimero_app_sdk::AppError>::into)? }
    } else {
        call
    };
    let encode = if m.kind == Kind::Init {
        quote! { let _ = __out; ::calimero_app_sdk::__private::encode(&()) }
    } else {
        match wire(&m.ok) {
            Some(Wire::Bytes) => {
                quote! { ::calimero_app_sdk::__private::encode(&::calimero_app_sdk::__private::Base64Url(&__out)) }
            }
            Some(Wire::OptBytes) => {
                quote! { ::calimero_app_sdk::__private::encode(&__out.as_deref().map(::calimero_app_sdk::__private::Base64Url)) }
            }
            Some(Wire::U64 | Wire::I64) => {
                quote! { ::calimero_app_sdk::__private::encode(&::calimero_app_sdk::__private::Decimal(__out)) }
            }
            Some(Wire::OptU64 | Wire::OptI64) => {
                quote! { ::calimero_app_sdk::__private::encode(&__out.map(::calimero_app_sdk::__private::Decimal)) }
            }
            None => quote! { ::calimero_app_sdk::__private::encode(&__out) },
        }
    };
    quote! {
        #index => {
            #[derive(::calimero_app_sdk::__private::serde::Deserialize)]
            #[serde(crate = "::calimero_app_sdk::__private::serde", deny_unknown_fields)]
            struct __Args { #(#fields,)* }
            let __Args { #(#names),* } = ::calimero_app_sdk::__private::decode_args::<__Args>(args)?;
            let __out = #call;
            #encode
        }
    }
}

pub fn expand(mut item: ItemImpl) -> syn::Result<TokenStream> {
    if item.trait_.is_some() || !item.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.self_ty,
            "#[app::logic] goes on a plain `impl State { .. }` block",
        ));
    }
    let state = (*item.self_ty).clone();
    let mut methods = Vec::new();
    for impl_item in &mut item.items {
        if let ImplItem::Fn(f) = impl_item {
            if let Some(m) = method(f)? {
                methods.push(m);
            }
        }
    }
    let inits = methods.iter().filter(|m| m.kind == Kind::Init).count();
    if inits > 1 {
        return Err(syn::Error::new_spanned(
            &state,
            "at most one `#[app::init]`",
        ));
    }

    let mut abi_methods = Map::new();
    for m in &methods {
        if abi_methods.insert(m.abi.clone(), abi_entry(m)).is_some() {
            return Err(syn::Error::new_spanned(
                &m.rust,
                format!("two methods are named `{}` in the ABI", m.abi),
            ));
        }
    }
    // Top-level keys in sorted order, like every object below them (`canonical`).
    let head = format!("{{\"abiVersion\":{ABI_VERSION},\"errors\":");
    let methods_json = format!(
        ",\"methods\":{},\"state\":",
        canonical(&Value::Object(abi_methods))
    );
    let tail = format!(",\"world\":\"{WORLD}\"}}");

    let specs = methods.iter().map(|m| {
        let name = &m.abi;
        let kind = match m.kind {
            Kind::Init => quote!(Init),
            Kind::Call => quote!(Call),
            Kind::View => quote!(View),
            Kind::Timer => quote!(Timer),
        };
        let public = m.attrs.public;
        let allow_system = m.attrs.allow_system;
        let apps = &m.attrs.allow_apps;
        quote! {
            ::calimero_app_sdk::__private::Method {
                name: #name,
                kind: ::calimero_app_sdk::__private::MethodKind::#kind,
                public: #public,
                allow_system: #allow_system,
                allow_apps: &[#(#apps),*],
            }
        }
    });
    let arms = methods
        .iter()
        .enumerate()
        .map(|(i, m)| run_arm(i, m, &state));
    let export = syn::Ident::new("__CalimeroExport", Span::call_site());

    Ok(quote! {
        #item

        impl ::calimero_app_sdk::__private::Dispatch for #state {
            const METHODS: &'static [::calimero_app_sdk::__private::Method] = &[#(#specs),*];

            fn run(
                index: usize,
                args: &[u8],
            ) -> ::core::result::Result<::std::vec::Vec<u8>, ::calimero_app_sdk::AppError> {
                match index {
                    #(#arms)*
                    _ => ::core::result::Result::Err(::calimero_app_sdk::AppError::new("UnknownMethod")),
                }
            }
        }

        #[allow(unused_imports)]
        use ::calimero_app_sdk::__private::abi_defaults::*;

        const _: () = {
            const __PARTS: &[&str] = &[
                #head,
                __CALIMERO_ABI_ERRORS,
                ",\"events\":",
                __CALIMERO_ABI_EVENTS,
                #methods_json,
                <#state as ::calimero_app_sdk::app::AppState>::ABI,
                #tail,
            ];
            const __LEN: usize = ::calimero_app_sdk::__private::abi_len(__PARTS);

            /// The app's ABI, lifted to the component's top level by the build.
            #[cfg_attr(target_arch = "wasm32", link_section = "calimero:abi")]
            #[used]
            static __CALIMERO_ABI: [u8; __LEN] = ::calimero_app_sdk::__private::abi_concat::<__LEN>(__PARTS);

            #[cfg(target_arch = "wasm32")]
            type #export = ::calimero_app_sdk::__private::Export<#state>;
            #[cfg(target_arch = "wasm32")]
            ::calimero_app_sdk::__private::bindings::__calimero_export_handle!(
                #export with_types_in ::calimero_app_sdk::__private::bindings
            );
        };
    }
    .into_token_stream())
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    fn lit(s: &str) -> String {
        proc_macro2::Literal::string(s).to_string()
    }

    #[test]
    fn methods_abi_is_sorted_whatever_serde_json_features() {
        let item: ItemImpl = parse_quote! {
            impl S {
                pub fn zed(&mut self, b_arg: u32, a_arg: String) -> Result<()> { Ok(()) }
                #[app::view]
                pub fn alpha(&self) -> Result<u32> { Ok(1) }
                #[app::every("30s")]
                pub fn tick(&mut self) -> Result<()> { Ok(()) }
            }
        };
        let out = expand(item).unwrap().to_string();
        let methods = concat!(
            r#",\"methods\":"#,
            r#"{"alpha":{"allowSystem":false,"confirm":false,"kind":"view","params":[],"public":false,"returns":"u32"},"#,
            r#""tick":{"allowSystem":true,"confirm":false,"every":"30s","kind":"timer","params":[],"public":false,"returns":null},"#,
            r#""zed":{"allowSystem":false,"confirm":false,"kind":"call","params":[["bArg","u32"],["aArg","string"]],"public":false,"returns":null}}"#,
            r#","state":"#
        );
        let methods = methods.replace(r#"\""#, "\"");
        assert!(out.contains(&lit(&methods)), "{out}");
    }

    fn refused(item: ItemImpl) -> String {
        match expand(item) {
            Ok(_) => panic!("expected a compile error"),
            Err(e) => e.to_string(),
        }
    }

    #[test]
    fn nested_wide_integers_are_refused() {
        let msg = refused(parse_quote! {
            impl S { pub fn m(&self, xs: Vec<u64>) -> Result<()> { Ok(()) } }
        });
        assert!(msg.contains("D20"), "{msg}");
        let msg = refused(parse_quote! {
            impl S { pub fn m(&self) -> Result<Option<Vec<i64>>> { Ok(None) } }
        });
        assert!(msg.contains("D20"), "{msg}");
        let msg = refused(parse_quote! {
            impl S { pub fn m(&self, p: (u32, u64)) -> Result<()> { Ok(()) } }
        });
        assert!(msg.contains("D20"), "{msg}");
        let msg = refused(parse_quote! {
            impl S { pub fn m(&self, p: [u64; 2]) -> Result<()> { Ok(()) } }
        });
        assert!(msg.contains("D20"), "{msg}");
        let msg = refused(parse_quote! {
            impl S { pub fn m(&self, p: Vec<Vec<u8>>) -> Result<()> { Ok(()) } }
        });
        assert!(msg.contains("D20"), "{msg}");
    }

    #[test]
    fn bad_rate_and_init_with_apps_are_refused() {
        let msg = refused(parse_quote! {
            impl S { #[app::public(rate = "ten")] pub fn m(&self) -> Result<()> { Ok(()) } }
        });
        assert!(msg.contains("rate"), "{msg}");
        let msg = refused(parse_quote! {
            impl S { #[app::init] #[app::allow(app = "com.x")] pub fn init() -> Result<Self> { todo!() } }
        });
        assert!(msg.contains("init"), "{msg}");
        let _ = expand(parse_quote! {
            impl S { #[app::view] #[app::public(rate = "10/min")] pub fn m(&self) -> Result<()> { Ok(()) } }
        })
        .unwrap();
    }
}
