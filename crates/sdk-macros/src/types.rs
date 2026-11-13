//! Names and types as the ABI writes them: camelCase names, ABI type strings, and the
//! 64-bit integer fields that travel as decimal strings in JSON (D20).

use quote::ToTokens;
use serde_json::Value;
use syn::{GenericArgument, PathArguments, Type};

/// `room_id` → `roomId`; `r#type` → `type`. Leading underscores are kept.
pub fn camel(snake: &str) -> String {
    let snake = snake.strip_prefix("r#").unwrap_or(snake);
    let lead = snake.len() - snake.trim_start_matches('_').len();
    let mut out = String::from(&snake[..lead]);
    let mut upper = false;
    for c in snake[lead..].chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// A field type JSON writes differently from serde's default: 64-bit integers as decimal
/// strings and bytes as base64url (D20), plain or optional.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wire {
    U64,
    I64,
    OptU64,
    OptI64,
    Bytes,
    OptBytes,
}

impl Wire {
    /// The SDK's `json` module for this type in JSON-only positions (args).
    pub fn json_module(self) -> String {
        match self {
            Self::U64 => crate::sdk::string("::json::u64_str"),
            Self::I64 => crate::sdk::string("::json::i64_str"),
            Self::OptU64 => crate::sdk::string("::json::opt_u64_str"),
            Self::OptI64 => crate::sdk::string("::json::opt_i64_str"),
            Self::Bytes => crate::sdk::string("::json::bytes_b64url"),
            Self::OptBytes => crate::sdk::string("::json::opt_bytes_b64url"),
        }
    }

    /// The module for record fields: the JSON form in human-readable formats, the native
    /// CBOR form (integer, byte string) otherwise.
    pub fn auto_module(self) -> String {
        match self {
            Self::U64 => crate::sdk::string("::json::u64_auto"),
            Self::I64 => crate::sdk::string("::json::i64_auto"),
            Self::OptU64 => crate::sdk::string("::json::opt_u64_auto"),
            Self::OptI64 => crate::sdk::string("::json::opt_i64_auto"),
            Self::Bytes => crate::sdk::string("::json::bytes_auto"),
            Self::OptBytes => crate::sdk::string("::json::opt_bytes_auto"),
        }
    }

    pub fn is_option(self) -> bool {
        matches!(self, Self::OptU64 | Self::OptI64 | Self::OptBytes)
    }

    pub fn is_bytes(self) -> bool {
        matches!(self, Self::Bytes | Self::OptBytes)
    }
}

/// The single type argument of `Name<T>`, when `ty` is a path ending in `name`.
pub fn generic_arg<'t>(ty: &'t Type, name: &str) -> Option<&'t Type> {
    let Type::Path(path) = ty else { return None };
    let last = path.path.segments.last()?;
    if last.ident != name {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &last.arguments else {
        return None;
    };
    args.args.iter().find_map(|a| match a {
        GenericArgument::Type(t) => Some(t),
        _ => None,
    })
}

fn ident_of(ty: &Type) -> Option<String> {
    let Type::Path(path) = ty else { return None };
    if path.qself.is_some() {
        return None;
    }
    let last = path.path.segments.last()?;
    matches!(last.arguments, PathArguments::None).then(|| last.ident.to_string())
}

fn is_vec_u8(ty: &Type) -> bool {
    generic_arg(ty, "Vec").is_some_and(|inner| ident_of(inner).as_deref() == Some("u8"))
}

fn wire_plain(ty: &Type) -> Option<Wire> {
    if is_vec_u8(ty) {
        return Some(Wire::Bytes);
    }
    match ident_of(ty).as_deref() {
        Some("u64") => Some(Wire::U64),
        Some("i64") => Some(Wire::I64),
        _ => None,
    }
}

/// Whether `ty` is `u64`, `i64`, `Vec<u8>` or an `Option` of one.
pub fn wire(ty: &Type) -> Option<Wire> {
    if let Some(plain) = wire_plain(ty) {
        return Some(plain);
    }
    match wire_plain(generic_arg(ty, "Option")?)? {
        Wire::U64 => Some(Wire::OptU64),
        Wire::I64 => Some(Wire::OptI64),
        Wire::Bytes => Some(Wire::OptBytes),
        _ => None,
    }
}

/// The types directly inside `ty`: generic arguments, tuple, array and slice elements.
fn inner_types(ty: &Type) -> Vec<&Type> {
    match ty {
        Type::Reference(r) => vec![&*r.elem],
        Type::Paren(p) => vec![&*p.elem],
        Type::Group(g) => vec![&*g.elem],
        Type::Array(a) => vec![&*a.elem],
        Type::Slice(s) => vec![&*s.elem],
        Type::Tuple(t) => t.elems.iter().collect(),
        Type::Path(p) => p
            .path
            .segments
            .iter()
            .flat_map(|seg| match &seg.arguments {
                PathArguments::AngleBracketed(args) => args
                    .args
                    .iter()
                    .filter_map(|a| match a {
                        GenericArgument::Type(t) => Some(t),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Refuse a `u64`, `i64` or `Vec<u8>` the macros cannot convert because it sits inside
/// another type (Ruling 27): JSON would get a number that loses precision, or an array of
/// bytes, instead of the D20 form. Types defined elsewhere (records, structs with plain
/// serde, aliases) cannot be inspected and are not checked.
pub fn check_wire(ty: &Type) -> syn::Result<()> {
    if wire(ty).is_some() {
        return Ok(());
    }
    fn walk(outer: &Type, ty: &Type) -> syn::Result<()> {
        for inner in inner_types(ty) {
            if let Some(w) = wire_plain(inner) {
                let what = if w.is_bytes() { "Vec<u8>" } else { "u64/i64" };
                let what = match ident_of(inner) {
                    Some(name) if !w.is_bytes() => name,
                    _ => what.to_owned(),
                };
                let effect = if w.is_bytes() {
                    "would not be base64url"
                } else {
                    "would lose precision"
                };
                return Err(syn::Error::new_spanned(
                    outer,
                    format!(
                        "{what} inside {} {effect} in JSON (D20); wrap it in a Record or use a string",
                        compact(outer)
                    ),
                ));
            }
            walk(outer, inner)?;
        }
        Ok(())
    }
    walk(ty, ty)
}

/// `"<n>/s"`, `"<n>/min"` or `"<n>/h"` with `n > 0`: a public method's rate limit.
pub fn valid_rate(text: &str) -> bool {
    let Some((count, unit)) = text.split_once('/') else {
        return false;
    };
    matches!(unit, "s" | "min" | "h")
        && !count.starts_with('0')
        && count.bytes().all(|b| b.is_ascii_digit())
        && count.parse::<u32>().is_ok_and(|n| n > 0)
}

/// Whether `ty` is `()`.
pub fn is_unit(ty: &Type) -> bool {
    matches!(ty, Type::Tuple(t) if t.elems.is_empty())
}

/// The ABI name of a Rust type: integer and float names as is, `string`, `bool`, `bytes`,
/// `option<T>`, `list<T>`, `tuple<A, B>`; any other type by its name.
pub fn abi_type(ty: &Type) -> String {
    match ty {
        Type::Reference(r) => abi_type(&r.elem),
        Type::Paren(p) => abi_type(&p.elem),
        Type::Group(g) => abi_type(&g.elem),
        Type::Slice(s) if ident_of(&s.elem).as_deref() == Some("u8") => "bytes".into(),
        Type::Slice(s) => format!("list<{}>", abi_type(&s.elem)),
        Type::Tuple(t) if t.elems.is_empty() => "unit".into(),
        Type::Tuple(t) => {
            let parts: Vec<_> = t.elems.iter().map(abi_type).collect();
            format!("tuple<{}>", parts.join(", "))
        }
        Type::Path(_) => path_type(ty),
        other => compact(other),
    }
}

/// The ABI name of a Rust type as plain serde writes it (`#[abi(plain)]`): as [`abi_type`],
/// but at any depth `u64`/`i64` are `u64_number`/`i64_number` (JSON numbers) and `Vec<u8>`/`[u8]`
/// `byte_list` (an array of numbers), where D20 would write decimal strings and base64url.
pub fn plain_abi_type(ty: &Type) -> String {
    match ty {
        Type::Reference(r) => plain_abi_type(&r.elem),
        Type::Paren(p) => plain_abi_type(&p.elem),
        Type::Group(g) => plain_abi_type(&g.elem),
        Type::Slice(s) if ident_of(&s.elem).as_deref() == Some("u8") => "byte_list".into(),
        Type::Slice(s) => format!("list<{}>", plain_abi_type(&s.elem)),
        Type::Array(a) => format!(
            "[{};{}]",
            plain_abi_type(&a.elem),
            a.len.to_token_stream().to_string().replace(' ', "")
        ),
        Type::Tuple(t) if t.elems.is_empty() => "unit".into(),
        Type::Tuple(t) => {
            let parts: Vec<_> = t.elems.iter().map(plain_abi_type).collect();
            format!("tuple<{}>", parts.join(", "))
        }
        Type::Path(p) if p.qself.is_none() => plain_path_type(ty, p),
        other => compact(other),
    }
}

fn plain_path_type(ty: &Type, path: &syn::TypePath) -> String {
    if let Some(name) = ident_of(ty) {
        return match name.as_str() {
            "u64" => "u64_number".into(),
            "i64" => "i64_number".into(),
            "String" | "str" => "string".into(),
            _ => name,
        };
    }
    if let Some(inner) = generic_arg(ty, "Option") {
        return format!("option<{}>", plain_abi_type(inner));
    }
    if let Some(inner) = generic_arg(ty, "Vec") {
        if ident_of(inner).as_deref() == Some("u8") {
            return "byte_list".into();
        }
        return format!("list<{}>", plain_abi_type(inner));
    }
    if let Some(inner) = generic_arg(ty, "Box") {
        return plain_abi_type(inner);
    }
    // Any other generic type (`BTreeMap<String, u64>`, `Page<u64>`): its name and its type
    // arguments, each as plain serde writes it.
    let Some(last) = path.path.segments.last() else {
        return compact(ty);
    };
    let PathArguments::AngleBracketed(args) = &last.arguments else {
        return compact(ty);
    };
    let args: Vec<String> = args
        .args
        .iter()
        .filter_map(|a| match a {
            GenericArgument::Type(t) => Some(plain_abi_type(t)),
            _ => None,
        })
        .collect();
    format!("{}<{}>", last.ident, args.join(","))
}

fn path_type(ty: &Type) -> String {
    if let Some(name) = ident_of(ty) {
        // Integers, floats and `bool` keep their Rust names; app types keep theirs.
        return match name.as_str() {
            "String" | "str" => "string".into(),
            _ => name,
        };
    }
    if let Some(inner) = generic_arg(ty, "Option") {
        return format!("option<{}>", abi_type(inner));
    }
    if let Some(inner) = generic_arg(ty, "Vec") {
        if ident_of(inner).as_deref() == Some("u8") {
            return "bytes".into();
        }
        return format!("list<{}>", abi_type(inner));
    }
    if let Some(inner) = generic_arg(ty, "Box") {
        return abi_type(inner);
    }
    compact(ty)
}

/// `value` as compact JSON with every object's keys sorted, whatever serde_json's
/// `preserve_order` feature (the build graph may unify it in), so the ABI bytes and the
/// bundle hash depend on the source only.
pub fn canonical(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            let parts: Vec<String> = entries
                .into_iter()
                .map(|(k, v)| format!("{}:{}", Value::String(k.clone()), canonical(v)))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(canonical).collect();
            format!("[{}]", parts.join(","))
        }
        other => other.to_string(),
    }
}

/// The type names serde and the ABI read without a shape of their own: they are spelled out
/// in the ABI type (`option<..>`, `list<..>`) or are primitives.
const SPELLED: &[&str] = &[
    "Option", "Vec", "VecDeque", "Box", "Rc", "Arc", "Cow", "BTreeMap", "HashMap", "IndexMap",
    "BTreeSet", "HashSet", "IndexSet", "String", "str", "bool", "char", "u8", "u16", "u32", "u64",
    "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize", "f32", "f64",
];

/// `ty` with every one of `params` (a generic type's parameters) replaced by `()`, so a
/// shape's dependency on `Wrapper<T>` names the concrete `Wrapper<()>`: the definition is
/// the same for every argument.
fn concrete(ty: &Type, params: &[syn::Ident]) -> Type {
    let mut ty = ty.clone();
    fn walk(ty: &mut Type, params: &[syn::Ident]) {
        match ty {
            Type::Path(p) if p.qself.is_none() => {
                if p.path.get_ident().is_some_and(|i| params.contains(i)) {
                    *ty = syn::parse_quote!(());
                    return;
                }
                for seg in &mut p.path.segments {
                    if let PathArguments::AngleBracketed(args) = &mut seg.arguments {
                        for arg in &mut args.args {
                            if let GenericArgument::Type(t) = arg {
                                walk(t, params);
                            }
                        }
                    }
                }
            }
            Type::Reference(r) => walk(&mut r.elem, params),
            Type::Paren(p) => walk(&mut p.elem, params),
            Type::Group(g) => walk(&mut g.elem, params),
            Type::Array(a) => walk(&mut a.elem, params),
            Type::Slice(s) => walk(&mut s.elem, params),
            Type::Tuple(t) => t.elems.iter_mut().for_each(|e| walk(e, params)),
            _ => {}
        }
    }
    walk(&mut ty, params);
    ty
}

/// The `__private::Probe::<T>::DEF` of every app type inside `ty`, for the ABI's `types`:
/// the type itself and its generic arguments, through references, tuples and arrays.
/// `skip` names what has no entry here: the generic parameters of the type being described,
/// its own name (a recursive field) and `Self`.
pub fn probes(
    ty: &Type,
    skip: &[syn::Ident],
    params: &[syn::Ident],
) -> Vec<proc_macro2::TokenStream> {
    let sdk = crate::sdk::path();
    let mut out = Vec::new();
    fn walk(
        ty: &Type,
        skip: &[syn::Ident],
        params: &[syn::Ident],
        sdk: &proc_macro2::TokenStream,
        out: &mut Vec<proc_macro2::TokenStream>,
    ) {
        match ty {
            Type::Path(p) if p.qself.is_none() => {
                let Some(last) = p.path.segments.last() else {
                    return;
                };
                let single = p.path.get_ident();
                if single.is_some_and(|i| i == "Self" || skip.contains(i) || params.contains(i)) {
                    return;
                }
                if !SPELLED.contains(&last.ident.to_string().as_str()) {
                    let concrete = concrete(ty, params);
                    out.push(quote::quote! { #sdk::__private::Probe::<#concrete>::DEF });
                }
                for t in inner_types(ty) {
                    walk(t, skip, params, sdk, out);
                }
            }
            Type::Path(_) => {}
            other => {
                for t in inner_types(other) {
                    walk(t, skip, params, sdk, out);
                }
            }
        }
    }
    walk(ty, skip, params, &sdk, &mut out);
    out
}

/// `probes` as one constant slice expression, with the fallback `NoShape` in scope.
pub fn probe_slice(probes: &[proc_macro2::TokenStream]) -> proc_macro2::TokenStream {
    let sdk = crate::sdk::path();
    quote::quote! {
        {
            #[allow(unused_imports)]
            use #sdk::__private::NoShape as _;
            &[#(#probes),*]
        }
    }
}

fn compact(ty: &Type) -> String {
    ty.to_token_stream().to_string().replace(' ', "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    #[test]
    fn camel_case_names() {
        assert_eq!(camel("room_id"), "roomId");
        assert_eq!(camel("release_no_shows"), "releaseNoShows");
        assert_eq!(camel("book"), "book");
        assert_eq!(camel("r#type"), "type");
        assert_eq!(camel("_hidden_x"), "_hiddenX");
    }

    #[test]
    fn abi_type_names() {
        assert_eq!(abi_type(&parse_quote!(String)), "string");
        assert_eq!(abi_type(&parse_quote!(Vec<u8>)), "bytes");
        assert_eq!(abi_type(&parse_quote!(Option<u64>)), "option<u64>");
        assert_eq!(abi_type(&parse_quote!(Vec<RoomId>)), "list<RoomId>");
        assert_eq!(abi_type(&parse_quote!((u32, bool))), "tuple<u32, bool>");
        assert_eq!(abi_type(&parse_quote!(Page<Booking>)), "Page<Booking>");
    }

    #[test]
    fn plain_abi_type_names() {
        assert_eq!(plain_abi_type(&parse_quote!(u64)), "u64_number");
        assert_eq!(
            plain_abi_type(&parse_quote!(Option<i64>)),
            "option<i64_number>"
        );
        assert_eq!(plain_abi_type(&parse_quote!(Vec<u8>)), "byte_list");
        assert_eq!(plain_abi_type(&parse_quote!(&[u8])), "byte_list");
        assert_eq!(plain_abi_type(&parse_quote!([u8; 32])), "[u8;32]");
        assert_eq!(plain_abi_type(&parse_quote!(Vec<u64>)), "list<u64_number>");
        assert_eq!(
            plain_abi_type(&parse_quote!(std::collections::BTreeMap<String, u64>)),
            "BTreeMap<string,u64_number>"
        );
        assert_eq!(
            plain_abi_type(&parse_quote!((u32, Vec<u8>))),
            "tuple<u32, byte_list>"
        );
        assert_eq!(plain_abi_type(&parse_quote!(Page<i64>)), "Page<i64_number>");
        assert_eq!(plain_abi_type(&parse_quote!(Room)), "Room");
        assert_eq!(plain_abi_type(&parse_quote!(u32)), "u32");
    }

    #[test]
    fn wire_fields() {
        assert_eq!(wire(&parse_quote!(u64)), Some(Wire::U64));
        assert_eq!(wire(&parse_quote!(Option<i64>)), Some(Wire::OptI64));
        assert_eq!(wire(&parse_quote!(Vec<u8>)), Some(Wire::Bytes));
        assert_eq!(wire(&parse_quote!(Option<Vec<u8>>)), Some(Wire::OptBytes));
        assert_eq!(wire(&parse_quote!(u32)), None);
        assert_eq!(wire(&parse_quote!(Vec<u64>)), None);
    }

    #[test]
    fn nested_wire_types_are_refused() {
        for ty in [
            parse_quote!(Vec<u64>),
            parse_quote!(Option<Vec<i64>>),
            parse_quote!(Option<Option<u64>>),
            parse_quote!((u32, u64)),
            parse_quote!([u64; 4]),
            parse_quote!(Vec<Vec<u8>>),
            parse_quote!(BTreeMap<String, u64>),
        ] {
            let ty: Type = ty;
            let err = check_wire(&ty).expect_err("refused");
            assert!(err.to_string().contains("(D20)"), "{err}");
        }
        assert_eq!(
            check_wire(&parse_quote!(Vec<u64>)).unwrap_err().to_string(),
            "u64 inside Vec<u64> would lose precision in JSON (D20); wrap it in a Record or use a string"
        );
        for ty in [
            parse_quote!(u64),
            parse_quote!(Option<Vec<u8>>),
            parse_quote!(Vec<u32>),
            parse_quote!(Page<Booking>),
            parse_quote!([u8; 32]),
        ] {
            let ty: Type = ty;
            check_wire(&ty).unwrap();
        }
    }

    #[test]
    fn canonical_json_sorts_keys() {
        let mut m = serde_json::Map::new();
        let _ = m.insert(
            "z".into(),
            serde_json::json!({ "b": 1, "a": [{ "d": 1, "c": 2 }] }),
        );
        let _ = m.insert("a".into(), serde_json::json!("x\"y"));
        assert_eq!(
            canonical(&Value::Object(m)),
            r#"{"a":"x\"y","z":{"a":[{"c":2,"d":1}],"b":1}}"#
        );
    }
}
