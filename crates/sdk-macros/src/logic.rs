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
    abi_type, camel, canonical, check_wire, generic_arg, is_unit, probe_slice, probes, valid_rate,
    wire, Wire,
};

/// The world the app component's ABI names.
const WORLD: &str = "calimero:app@0.5.7";
/// The world the compute component's ABI names.
const COMPUTE_WORLD: &str = "calimero:app/compute@0.5.7";
/// The ABI version this SDK writes.
const ABI_VERSION: u64 = 1;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Init,
    Call,
    View,
    Timer,
    Action,
    Compute,
}

/// `#[app::compute(..)]`: the caps the host enforces and the method that gets the outcome.
struct ComputeAttr {
    fuel: u64,
    memory_mib: Option<u32>,
    wall_secs: Option<u32>,
    result: String,
}

#[derive(Default)]
struct Attrs {
    init: bool,
    view: bool,
    action: bool,
    /// `allow(action)`: called only by the app's own actions.
    action_only: bool,
    confirm: bool,
    public: bool,
    rate: Option<String>,
    every: Option<String>,
    allow_system: bool,
    allow_apps: Vec<String>,
    compute: Option<ComputeAttr>,
    compute_result: bool,
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
    /// The method's `///` comment, for the people reading the ABI.
    doc: Option<String>,
    /// An `async fn`: its body runs under the SDK's xcall executor (`xcall::block_on`).
    is_async: bool,
}

/// The `app::<name>` attribute name, if `attr` is one (`app::x`, or `<sdk>::app::x` for the
/// SDK under either of its names).
fn app_attr(attr: &Attribute) -> Option<String> {
    let segs: Vec<String> = attr
        .path()
        .segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect();
    match segs.as_slice() {
        [app, name] if app == "app" => Some(name.clone()),
        [sdk, app, name] if crate::sdk::is_sdk(sdk) && app == "app" => Some(name.clone()),
        _ => None,
    }
}

/// The doc comments on `attrs`, one leading space per line removed; `None` when there are none.
/// Only string literals are read; `include_str!` and `concat!` are left out.
fn doc_comment(attrs: &[Attribute]) -> Option<String> {
    let text: Vec<String> = attrs
        .iter()
        .filter(|a| a.path().is_ident("doc"))
        .filter_map(|a| match &a.meta {
            syn::Meta::NameValue(nv) => doc_literal(&nv.value),
            _ => None,
        })
        .map(|raw| unindent(&raw))
        .collect();
    let doc = text.join("\n").trim().to_owned();
    (!doc.is_empty()).then_some(doc)
}

/// The string in a `#[doc = ..]` value; `macro_rules!` wraps its `$d` fragments in a group.
fn doc_literal(value: &syn::Expr) -> Option<String> {
    match value {
        syn::Expr::Group(g) => doc_literal(&g.expr),
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(s),
            ..
        }) => Some(s.value()),
        _ => None,
    }
}

/// A `///` is one line; a `/** */` is one string, whose ` * ` decoration goes when every
/// line after the first carries it, else the indentation they share (as rustdoc does).
fn unindent(raw: &str) -> String {
    let lines: Vec<&str> = raw.split('\n').collect();
    let rest = || lines[1..].iter().filter(|l| !l.trim().is_empty());
    let starred = rest().all(|l| l.trim_start().starts_with('*'));
    let indent = rest()
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let strip = |line: &str| line.strip_prefix(' ').unwrap_or(line).to_owned();
    let mut out = vec![strip(lines[0])];
    for line in &lines[1..] {
        out.push(if starred {
            strip(line.trim_start().strip_prefix('*').unwrap_or(""))
        } else {
            line.get(indent..).unwrap_or(line.trim_start()).to_owned()
        });
    }
    out.join("\n")
}

/// `"<n>s"`, `"<n>m"` or `"<n>h"` with `n > 0`, as the host reads it.
fn valid_every(text: &str) -> bool {
    let Some(unit) = text.chars().last() else {
        return false;
    };
    matches!(unit, 's' | 'm' | 'h') && text[..text.len() - 1].parse::<u64>().is_ok_and(|n| n > 0)
}

fn compute_attr(attr: &Attribute) -> syn::Result<ComputeAttr> {
    let (mut fuel, mut memory_mib, mut wall_secs, mut result) = (None, None, None, None);
    attr.parse_nested_meta(|meta| {
        let key = meta.path.get_ident().map(ToString::to_string);
        match key.as_deref() {
            Some("fuel") => fuel = Some(meta.value()?.parse::<syn::LitInt>()?.base10_parse()?),
            Some("memory_mib") => {
                memory_mib = Some(meta.value()?.parse::<syn::LitInt>()?.base10_parse()?);
            }
            Some("wall_secs") => {
                wall_secs = Some(meta.value()?.parse::<syn::LitInt>()?.base10_parse()?);
            }
            Some("result") => result = Some(meta.value()?.parse::<LitStr>()?.value()),
            _ => return Err(meta.error("expected `fuel`, `memory_mib`, `wall_secs` or `result`")),
        }
        Ok(())
    })?;
    let missing = |what: &str| syn::Error::new_spanned(attr, format!("`compute` needs `{what}`"));
    Ok(ComputeAttr {
        fuel: fuel.ok_or_else(|| missing("fuel = N"))?,
        memory_mib,
        wall_secs,
        result: result.ok_or_else(|| missing("result = \"method\""))?,
    })
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
            "action" => attrs.action = true,
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
                // A timer's caller is always `System`, so `every` implies it.
                attrs.allow_system = true;
            }
            "compute" => attrs.compute = Some(compute_attr(&attr)?),
            "compute_result" => {
                attrs.compute_result = true;
                // The node runs it as the system caller (a compute run's outcome).
                attrs.allow_system = true;
            }
            "allow" => attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("system") {
                    attrs.allow_system = true;
                    Ok(())
                } else if meta.path.is_ident("action") {
                    attrs.action_only = true;
                    Ok(())
                } else if meta.path.is_ident("app") {
                    let lit: LitStr = meta.value()?.parse()?;
                    attrs.allow_apps.push(lit.value());
                    Ok(())
                } else {
                    Err(meta.error("expected `system`, `action` or `app = \"<app id>\"`"))
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
        let any = attrs.view
            || attrs.confirm
            || attrs.public
            || attrs.every.is_some()
            || attrs.action
            || attrs.action_only
            || attrs.compute.is_some();
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
    if !sig.generics.params.is_empty() {
        return fail("app methods take no generics");
    }

    let mut receiver_mut = None;
    let mut params = Vec::new();
    // An action's first parameter is its context, `&mut ActionCtx`, not an argument.
    let mut inputs = sig.inputs.iter().peekable();
    if attrs.action {
        match inputs.next() {
            Some(FnArg::Typed(pt)) if matches!(&*pt.ty, Type::Reference(r) if r.mutability.is_some()) =>
                {}
            _ => return fail("an action takes `ctx: &mut ActionCtx` first and no `self`"),
        }
    }
    for arg in inputs {
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
                check_wire(&pt.ty).map_err(|e| {
                    syn::Error::new(e.span(), format!("parameter `{}`: {e}", pi.ident))
                })?;
                params.push((pi.ident.clone(), (*pt.ty).clone()));
            }
        }
    }

    let kind = match (
        attrs.init,
        attrs.view,
        attrs.every.is_some(),
        attrs.action,
        attrs.compute.is_some(),
    ) {
        (true, false, false, false, false) => Kind::Init,
        (false, true, false, false, false) => Kind::View,
        (false, false, true, false, false) => Kind::Timer,
        (false, false, false, true, false) => Kind::Action,
        (false, false, false, false, true) => Kind::Compute,
        (false, false, false, false, false) => Kind::Call,
        _ => {
            return fail("`init`, `view`, `every`, `action` and `compute` exclude each other");
        }
    };
    if sig.asyncness.is_some() && kind == Kind::Compute {
        return fail("a compute method calls no other app: it is not `async`");
    }
    if attrs.compute_result && kind != Kind::Call {
        return fail("`compute_result` goes on a plain call method");
    }
    if attrs.compute_result && attrs.public {
        return fail("a `compute_result` method is not public: only the node runs it");
    }
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
        Kind::Compute if receiver_mut.is_some() => {
            return fail("a compute method has no state: it takes no `self`")
        }
        Kind::Compute if attrs.confirm || attrs.public || attrs.allow_system => {
            return fail("a compute method is started by the app; it takes no access attribute")
        }
        Kind::Call if receiver_mut.is_none() => {
            return fail("an app method takes `&self` or `&mut self`")
        }
        Kind::Action if receiver_mut.is_some() => {
            return fail(
                "an action has no state access (it runs outside the log): it takes no `self`",
            )
        }
        Kind::Action if attrs.confirm || attrs.public || attrs.action_only => {
            return fail("an action takes no `confirm`, `public` or `allow(action)`")
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
        check_wire(&ok).map_err(|e| syn::Error::new(e.span(), format!("result: {e}")))?;
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
        doc: doc_comment(&f.attrs),
        is_async: sig.asyncness.is_some(),
    }))
}

fn abi_entry(m: &Method) -> Value {
    let kind = match m.kind {
        Kind::Init => "init",
        Kind::Call => "call",
        Kind::View => "view",
        Kind::Timer => "timer",
        Kind::Action => "action",
        Kind::Compute => "compute",
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
    if let Some(doc) = &m.doc {
        let _ = entry.insert("doc".into(), doc.clone().into());
    }
    if let Some(compute) = &m.attrs.compute {
        let mut caps = Map::new();
        let _ = caps.insert("fuel".into(), compute.fuel.into());
        if let Some(memory_mib) = compute.memory_mib {
            let _ = caps.insert("memoryMib".into(), memory_mib.into());
        }
        if let Some(wall_secs) = compute.wall_secs {
            let _ = caps.insert("wallSecs".into(), wall_secs.into());
        }
        let _ = caps.insert("result".into(), camel(&compute.result).into());
        let _ = entry.insert("compute".into(), Value::Object(caps));
    }
    if m.attrs.compute_result {
        let _ = entry.insert("computeResult".into(), true.into());
    }
    if let Some(every) = &m.attrs.every {
        let _ = entry.insert("every".into(), every.clone().into());
    }
    if let Some(rate) = &m.attrs.rate {
        let _ = entry.insert("rate".into(), rate.clone().into());
    }
    if !m.attrs.allow_apps.is_empty() {
        let _ = entry.insert("allowApps".into(), m.attrs.allow_apps.clone().into());
    }
    if m.attrs.action_only {
        let _ = entry.insert("allowAction".into(), true.into());
    }
    Value::Object(entry)
}

fn run_arm(index: usize, m: &Method, state: &Type) -> TokenStream {
    let sdk = crate::sdk::path();
    let serde_crate = crate::sdk::string("::__private::serde");
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
    // An `async fn` runs under the xcall executor, which batches the xcalls it awaits.
    let run = |call: TokenStream| {
        if m.is_async {
            quote! { #sdk::xcall::block_on(#call) }
        } else {
            call
        }
    };
    let call = match m.receiver_mut {
        None if m.kind == Kind::Action => {
            let call = run(quote! { <#state>::#rust(&mut __ctx, #(#names),*) });
            quote! {
                {
                    let mut __ctx = #sdk::ActionCtx::current();
                    #call
                }
            }
        }
        None => run(quote! { <#state>::#rust(#(#names),*) }),
        Some(_) => {
            let call = run(quote! { __state.#rust(#(#names),*) });
            quote! {
                {
                    #[allow(unused_mut)]
                    let mut __state = <#state as #sdk::app::AppState>::load();
                    #call
                }
            }
        }
    };
    let call = if m.returns_result {
        quote! { #call.map_err(::core::convert::Into::<#sdk::AppError>::into)? }
    } else {
        call
    };
    let encode = if m.kind == Kind::Init {
        quote! { let _ = __out; #sdk::__private::encode(&()) }
    } else {
        match wire(&m.ok) {
            Some(Wire::Bytes) => {
                quote! { #sdk::__private::encode(&#sdk::__private::Base64Url(&__out)) }
            }
            Some(Wire::OptBytes) => {
                quote! { #sdk::__private::encode(&__out.as_deref().map(#sdk::__private::Base64Url)) }
            }
            Some(Wire::U64 | Wire::I64) => {
                quote! { #sdk::__private::encode(&#sdk::__private::Decimal(__out)) }
            }
            Some(Wire::OptU64 | Wire::OptI64) => {
                quote! { #sdk::__private::encode(&__out.map(#sdk::__private::Decimal)) }
            }
            None => quote! { #sdk::__private::encode(&__out) },
        }
    };
    if m.attrs.compute_result {
        if let [(name, ty)] = m.params.as_slice() {
            // The node's outcome is one JSON object that the parameter takes whole.
            return quote! {
                #index => {
                    let #name: #ty = #sdk::__private::decode_args::<#ty>(args)?;
                    let __out = #call;
                    #encode
                }
            };
        }
    }
    quote! {
        #index => {
            #[derive(#sdk::__private::serde::Deserialize)]
            #[serde(crate = #serde_crate, deny_unknown_fields)]
            struct __Args { #(#fields,)* }
            let __Args { #(#names),* } = #sdk::__private::decode_args::<__Args>(args)?;
            let __out = #call;
            #encode
        }
    }
}

pub fn expand(mut item: ItemImpl) -> syn::Result<TokenStream> {
    let sdk = crate::sdk::path();
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
    for m in &methods {
        let Some(compute) = &m.attrs.compute else {
            continue;
        };
        let result = camel(&compute.result);
        if !methods
            .iter()
            .any(|r| r.abi == result && r.attrs.compute_result)
        {
            return Err(syn::Error::new_spanned(
                &m.rust,
                format!(
                    "`result = {:?}` is not a `#[app::compute_result]` method",
                    compute.result
                ),
            ));
        }
    }
    let (compute, app): (Vec<&Method>, Vec<&Method>) =
        methods.iter().partition(|m| m.kind == Kind::Compute);
    let app_abi = abi_parts(&state, &methods.iter().collect::<Vec<_>>(), WORLD);
    let compute_abi = abi_parts(&state, &compute, COMPUTE_WORLD);
    let app_dispatch = dispatch(&state, &app);
    let compute_dispatch = dispatch(&state, &compute);
    let export = syn::Ident::new("__CalimeroExport", Span::call_site());
    item.attrs.push(syn::parse_quote!(#[allow(dead_code)]));

    // The SDK picks the build: the compute component dispatches the compute methods only and
    // exports `run`; the app component dispatches the rest. The cfg lives in the SDK, never in
    // this output, so app crates need not declare it.
    Ok(quote! {
        #item

        #sdk::__calimero_app_only! { #app_dispatch }
        #sdk::__calimero_compute_only! { #compute_dispatch }

        #[allow(unused_imports)]
        use #sdk::__private::abi_defaults::*;

        const _: () = {
            #sdk::__calimero_app_only! {
                const __PARTS: &[&str] = &[#(#app_abi),*];
            }
            #sdk::__calimero_compute_only! {
                const __PARTS: &[&str] = &[#(#compute_abi),*];
            }
            const __LEN: usize = #sdk::__private::abi_len(__PARTS);

            /// The app's ABI, lifted to the component's top level by the build.
            #[cfg_attr(target_arch = "wasm32", link_section = "calimero:abi")]
            #[used]
            static __CALIMERO_ABI: [u8; __LEN] = #sdk::__private::abi_concat::<__LEN>(__PARTS);

            #[cfg(target_arch = "wasm32")]
            type #export = #sdk::__private::Export<#state>;
            #sdk::__calimero_app_only! {
                #[cfg(target_arch = "wasm32")]
                #sdk::__private::bindings::__calimero_export_handle!(
                    #export with_types_in #sdk::__private::bindings
                );
            }
            #sdk::__calimero_compute_only! {
                #[cfg(target_arch = "wasm32")]
                #sdk::__private::bindings::__calimero_export_run!(
                    #export with_types_in #sdk::__private::bindings
                );
            }
        };
    }
    .into_token_stream())
}

/// The ABI of a component holding `methods`, as the parts `abi_concat` joins; top-level keys
/// are in sorted order, like every object below them (`canonical`).
fn abi_parts(state: &Type, methods: &[&Method], world: &str) -> Vec<TokenStream> {
    let sdk = crate::sdk::path();
    let mut abi_methods = Map::new();
    for m in methods {
        let _ = abi_methods.insert(m.abi.clone(), abi_entry(m));
    }
    let head = format!("{{\"abiVersion\":{ABI_VERSION},\"errors\":");
    let methods_json = format!(
        ",\"methods\":{},\"state\":",
        canonical(&Value::Object(abi_methods))
    );
    let tail = format!(",\"world\":\"{world}\"}}");
    // Every app type the methods take or answer, then (through their shapes) the types
    // those refer to; with the errors' and events' fields, as `types`.
    let probes: Vec<TokenStream> = methods
        .iter()
        .flat_map(|m| {
            m.params
                .iter()
                .map(|(_, ty)| ty)
                .chain([&m.ok])
                .flat_map(|ty| probes(ty, &[], &[]))
                .collect::<Vec<_>>()
        })
        .collect();
    let roots = probe_slice(&probes);
    let types = quote! {
        {
            // Only `TYPES.as_str()` reaches the binary, as the ABI's bytes.
            #[allow(clippy::large_const_arrays, reason = "evaluated at compile time only")]
            const TYPES: #sdk::__private::TypesJson = #sdk::__private::types_json(&[
                #roots,
                __CALIMERO_ABI_ERROR_TYPES,
                __CALIMERO_ABI_EVENT_TYPES,
            ]);
            TYPES.as_str()
        }
    };
    vec![
        quote!(#head),
        quote!(__CALIMERO_ABI_ERRORS),
        quote!(",\"events\":"),
        quote!(__CALIMERO_ABI_EVENTS),
        quote!(#methods_json),
        quote!(<#state as #sdk::app::AppState>::ABI),
        types,
        quote!(#tail),
    ]
}

/// `Dispatch` for the state type over `methods`, indexed in their order.
fn dispatch(state: &Type, methods: &[&Method]) -> TokenStream {
    let sdk = crate::sdk::path();
    let specs = methods.iter().map(|m| {
        let name = &m.abi;
        let kind = match m.kind {
            Kind::Init => quote!(Init),
            Kind::Call => quote!(Call),
            Kind::View => quote!(View),
            Kind::Timer => quote!(Timer),
            Kind::Action => quote!(Action),
            Kind::Compute => quote!(Compute),
        };
        let public = m.attrs.public;
        let allow_system = m.attrs.allow_system;
        let apps = &m.attrs.allow_apps;
        quote! {
            #sdk::__private::Method {
                name: #name,
                kind: #sdk::__private::MethodKind::#kind,
                public: #public,
                allow_system: #allow_system,
                allow_apps: &[#(#apps),*],
            }
        }
    });
    let arms = methods
        .iter()
        .enumerate()
        .map(|(i, m)| run_arm(i, m, state));
    quote! {
        impl #sdk::__private::Dispatch for #state {
            const METHODS: &'static [#sdk::__private::Method] = &[#(#specs),*];

            fn run(
                index: usize,
                args: &[u8],
            ) -> ::core::result::Result<::std::vec::Vec<u8>, #sdk::AppError> {
                match index {
                    #(#arms)*
                    _ => ::core::result::Result::Err(#sdk::AppError::new("UnknownMethod")),
                }
            }
        }
    }
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

    #[test]
    fn method_doc_comments_reach_the_abi() {
        let item: ItemImpl = parse_quote! {
            impl S {
                /// Deletes the workspace.
                ///
                /// Everyone loses access.
                #[doc(hidden)]
                pub fn delete_workspace(&mut self) -> Result<()> { Ok(()) }
                #[app::view]
                pub fn bare(&self) -> Result<u32> { Ok(1) }
            }
        };
        let out = expand(item).unwrap().to_string().replace(r#"\""#, "\"");
        let documented = r#""deleteWorkspace":{"allowSystem":false,"confirm":false,"doc":"Deletes the workspace.\\n\\nEveryone loses access.","kind":"call""#;
        assert!(out.contains(documented), "{out}");
        assert!(
            out.contains(r#""bare":{"allowSystem":false,"confirm":false,"kind":"view""#),
            "{out}"
        );
    }

    fn doc_of(f: ImplItemFn) -> Option<String> {
        doc_comment(&f.attrs)
    }

    #[test]
    fn doc_comment_reads_a_macro_rules_grouped_literal() {
        let literal: syn::Expr = parse_quote!("Lists the rooms.");
        let grouped = Attribute {
            pound_token: Default::default(),
            style: syn::AttrStyle::Outer,
            bracket_token: Default::default(),
            meta: syn::Meta::NameValue(syn::MetaNameValue {
                path: parse_quote!(doc),
                eq_token: Default::default(),
                value: syn::Expr::Group(syn::ExprGroup {
                    attrs: Vec::new(),
                    group_token: Default::default(),
                    expr: Box::new(literal),
                }),
            }),
        };
        assert_eq!(doc_comment(&[grouped]).as_deref(), Some("Lists the rooms."));
    }

    #[test]
    fn doc_comment_leaves_out_what_a_proc_macro_cannot_read() {
        let f: ImplItemFn = parse_quote! {
            #[doc = include_str!("rooms.txt")]
            #[doc = concat!("a", "b")]
            pub fn rooms(&self) {}
        };
        assert_eq!(doc_of(f), None);
        let f: ImplItemFn = parse_quote! {
            #[doc = include_str!("rooms.txt")]
            /// Lists the rooms.
            pub fn rooms(&self) {}
        };
        assert_eq!(doc_of(f).as_deref(), Some("Lists the rooms."));
    }

    #[test]
    fn doc_comment_reads_a_block_comment() {
        let f: ImplItemFn = parse_quote! {
            /**
             * Lists the rooms.
             *
             *   Indented.
             */
            pub fn rooms(&self) {}
        };
        assert_eq!(
            doc_of(f).as_deref(),
            Some("Lists the rooms.\n\n  Indented.")
        );
        let f: ImplItemFn = parse_quote! {
            #[doc = " Lists the rooms.\n  - one\n  * two\n"]
            pub fn rooms(&self) {}
        };
        assert_eq!(doc_of(f).as_deref(), Some("Lists the rooms.\n- one\n* two"));
    }

    #[test]
    fn doc_comment_drops_the_shared_indent_of_an_undecorated_block() {
        let f: ImplItemFn = parse_quote! {
            #[doc = " Lists.\n        More.\n          Deeper.\n    "]
            pub fn rooms(&self) {}
        };
        assert_eq!(doc_of(f).as_deref(), Some("Lists.\nMore.\n  Deeper."));
    }

    #[test]
    fn doc_comment_drops_stars_from_a_literal_that_looks_like_a_block_comment() {
        let f: ImplItemFn = parse_quote! {
            #[doc = " Lists.\n * a\n * b"]
            pub fn rooms(&self) {}
        };
        assert_eq!(doc_of(f).as_deref(), Some("Lists.\na\nb"));
    }

    #[test]
    fn an_async_method_runs_under_the_xcall_executor_and_a_compute_one_is_refused() {
        let item: ItemImpl = parse_quote! {
            impl S {
                pub async fn both(&mut self, a: String) -> app::Result<u32> { Ok(1) }
                #[app::view]
                pub async fn look(&self) -> app::Result<u32> { Ok(1) }
                pub fn plain(&mut self) -> app::Result<()> { Ok(()) }
            }
        };
        let out = expand(item).unwrap().to_string();
        assert_eq!(out.matches("xcall :: block_on").count(), 2, "{out}");
        let msg = refused(parse_quote! {
            impl S {
                #[app::compute(fuel = 1, result = "done")]
                pub async fn work() -> app::Result<()> { Ok(()) }
                #[app::compute_result]
                pub fn done(&mut self, o: String) -> app::Result<()> { Ok(()) }
            }
        });
        assert!(msg.contains("not `async`"), "{msg}");
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
        assert!(msg.contains("JSON"), "{msg}");
        let msg = refused(parse_quote! {
            impl S { pub fn m(&self) -> Result<Option<Vec<i64>>> { Ok(None) } }
        });
        assert!(msg.contains("JSON"), "{msg}");
        let msg = refused(parse_quote! {
            impl S { pub fn m(&self, p: (u32, u64)) -> Result<()> { Ok(()) } }
        });
        assert!(msg.contains("JSON"), "{msg}");
        let msg = refused(parse_quote! {
            impl S { pub fn m(&self, p: [u64; 2]) -> Result<()> { Ok(()) } }
        });
        assert!(msg.contains("JSON"), "{msg}");
        let msg = refused(parse_quote! {
            impl S { pub fn m(&self, p: Vec<Vec<u8>>) -> Result<()> { Ok(()) } }
        });
        assert!(msg.contains("JSON"), "{msg}");
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

    #[test]
    fn compute_methods_and_their_result_reach_the_abi() {
        let item: ItemImpl = parse_quote! {
            impl S {
                #[app::compute(fuel = 100, memory_mib = 64, result = "on_done")]
                pub fn digest(x: u32) -> Result<()> { Ok(()) }
                #[app::compute_result]
                pub fn on_done(&mut self) -> Result<()> { Ok(()) }
            }
        };
        let out = expand(item).unwrap().to_string();
        let digest = r#""digest":{"allowSystem":false,"compute":{"fuel":100,"memoryMib":64,"result":"onDone"},"confirm":false,"kind":"compute","params":[["x","u32"]],"public":false,"returns":null}"#;
        let result = r#""onDone":{"allowSystem":true,"computeResult":true,"confirm":false,"kind":"call","params":[],"public":false,"returns":null}"#;
        let out = out.replace(r#"\""#, "\"");
        assert!(out.contains(digest), "{out}");
        assert!(out.contains(result), "{out}");
    }

    #[test]
    fn the_compute_abi_lists_compute_methods_only_in_the_compute_world() {
        let item: ItemImpl = parse_quote! {
            impl S {
                #[app::compute(fuel = 100, result = "on_done")]
                pub fn digest(x: u32) -> Result<()> { Ok(()) }
                #[app::compute_result]
                pub fn on_done(&mut self) -> Result<()> { Ok(()) }
            }
        };
        let out = expand(item).unwrap().to_string().replace(r#"\""#, "\"");
        let compute_methods = r#",\"methods\":{"digest":{"allowSystem":false,"compute":{"fuel":100,"result":"onDone"},"confirm":false,"kind":"compute","params":[["x","u32"]],"public":false,"returns":null}},\"state\":"#
            .replace(r#"\""#, "\"");
        assert!(out.contains(&compute_methods), "{out}");
        assert!(
            out.contains(
                r#",\"world\":\"calimero:app/compute@0.5.7\"}"#.replace(r#"\""#, "\"").as_str()
            ),
            "{out}"
        );
        assert!(
            out.contains(r#",\"world\":\"calimero:app@0.5.7\"}"#.replace(r#"\""#, "\"").as_str()),
            "{out}"
        );
    }

    #[test]
    fn bad_compute_declarations_are_refused() {
        let msg = refused(parse_quote! {
            impl S { #[app::compute(result = "r")] pub fn m() -> Result<()> { Ok(()) } }
        });
        assert!(msg.contains("fuel"), "{msg}");
        let msg = refused(parse_quote! {
            impl S { #[app::compute(fuel = 1)] pub fn m() -> Result<()> { Ok(()) } }
        });
        assert!(msg.contains("result"), "{msg}");
        let msg = refused(parse_quote! {
            impl S { #[app::compute(fuel = 1, speed = 2, result = "r")] pub fn m() -> Result<()> { Ok(()) } }
        });
        assert!(msg.contains("expected"), "{msg}");
        let msg = refused(parse_quote! {
            impl S { #[app::compute(fuel = 1, result = "r")] pub fn m(&self) -> Result<()> { Ok(()) } }
        });
        assert!(msg.contains("no `self`"), "{msg}");
        let msg = refused(parse_quote! {
            impl S { #[app::compute(fuel = 1, result = "r")] #[app::view] pub fn m() -> Result<()> { Ok(()) } }
        });
        assert!(msg.contains("exclude"), "{msg}");
        let msg = refused(parse_quote! {
            impl S { #[app::compute(fuel = 1, result = "missing")] pub fn m() -> Result<()> { Ok(()) } }
        });
        assert!(msg.contains("compute_result"), "{msg}");
        let msg = refused(parse_quote! {
            impl S { #[app::compute_result] #[app::view] pub fn m(&self) -> Result<()> { Ok(()) } }
        });
        assert!(
            msg.contains("compute_result") || msg.contains("exclude"),
            "{msg}"
        );
        let msg = refused(parse_quote! {
            impl S { #[app::compute_result] #[app::public] pub fn m(&mut self) -> Result<()> { Ok(()) } }
        });
        assert!(msg.contains("not public"), "{msg}");
    }
}
