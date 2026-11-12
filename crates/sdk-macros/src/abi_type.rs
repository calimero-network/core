//! `#[derive(AbiType)]`: a serde struct's or enum's JSON shape, for the ABI's `types`.
//!
//! The derive reads the `#[serde(..)]` attributes that change the JSON (`rename`,
//! `rename_all`, `rename_all_fields`, `tag`, `content`, `untagged`, `transparent`, `skip`,
//! `skip_serializing`, `skip_serializing_if`, `with`) and refuses the ones whose JSON it cannot
//! know (`flatten`, `into`, `from`, `try_from`, `remote`, a `with` other than the SDK's D20
//! helpers). A field that `skip_serializing_if` may leave out, or that `default` (on the field
//! or the struct) lets a caller leave out, is optional in the shape.
//!
//! A `u64`, `i64` or `Vec<u8>` field says which JSON it travels as: its D20 form (`#[serde(with =
//! "calimero_sdk::json::u64_str")]`, ...: a decimal string, base64url), or, marked
//! `#[abi(plain)]` (on the field, or on the type for every field), what plain serde writes: a
//! JSON number (`u64_number`, `i64_number`, which JavaScript reads exactly only up to 2^53) and
//! an array of numbers (`byte_list`), at any depth (`Vec<u64>` is `list<u64_number>`). Unmarked,
//! it is refused (Ruling 27): the ABI's `u64` and `bytes` mean the strings.

use proc_macro2::TokenStream;
use quote::quote;
use serde_json::{json, Value};
use syn::spanned::Spanned;
use syn::{Attribute, Data, DeriveInput, Fields, Ident, LitStr};

use crate::types::{abi_type, canonical, check_wire, plain_abi_type, probe_slice, probes, wire};

/// serde's `rename_all` rules.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Rule {
    Lower,
    Upper,
    Pascal,
    Camel,
    Snake,
    ScreamingSnake,
    Kebab,
    ScreamingKebab,
}

impl Rule {
    fn parse(lit: &LitStr) -> syn::Result<Self> {
        Ok(match lit.value().as_str() {
            "lowercase" => Self::Lower,
            "UPPERCASE" => Self::Upper,
            "PascalCase" => Self::Pascal,
            "camelCase" => Self::Camel,
            "snake_case" => Self::Snake,
            "SCREAMING_SNAKE_CASE" => Self::ScreamingSnake,
            "kebab-case" => Self::Kebab,
            "SCREAMING-KEBAB-CASE" => Self::ScreamingKebab,
            _ => return Err(syn::Error::new(lit.span(), "unknown rename_all rule")),
        })
    }

    /// A variant's name (PascalCase in Rust), as serde renames it.
    fn variant(self, name: &str) -> String {
        match self {
            Self::Pascal => name.to_owned(),
            Self::Lower => name.to_ascii_lowercase(),
            Self::Upper => name.to_ascii_uppercase(),
            Self::Camel => {
                let mut chars = name.chars();
                chars
                    .next()
                    .map(|c| c.to_ascii_lowercase().to_string() + chars.as_str())
                    .unwrap_or_default()
            }
            Self::Snake | Self::ScreamingSnake | Self::Kebab | Self::ScreamingKebab => {
                let mut snake = String::new();
                for (i, c) in name.char_indices() {
                    if i > 0 && c.is_uppercase() {
                        snake.push('_');
                    }
                    snake.push(c.to_ascii_lowercase());
                }
                from_snake(&snake, self)
            }
        }
    }

    /// A field's name (snake_case in Rust), as serde renames it.
    fn field(self, name: &str) -> String {
        from_snake(name, self)
    }
}

/// A snake_case name under `rule`.
fn from_snake(snake: &str, rule: Rule) -> String {
    match rule {
        Rule::Lower | Rule::Snake => snake.to_owned(),
        Rule::Upper | Rule::ScreamingSnake => snake.to_ascii_uppercase(),
        Rule::Kebab => snake.replace('_', "-"),
        Rule::ScreamingKebab => snake.to_ascii_uppercase().replace('_', "-"),
        Rule::Pascal | Rule::Camel => {
            let mut out = String::new();
            let mut upper = rule == Rule::Pascal;
            for c in snake.chars() {
                if c == '_' {
                    upper = true;
                } else if upper {
                    out.push(c.to_ascii_uppercase());
                    upper = false;
                } else {
                    out.push(c);
                }
            }
            out
        }
    }
}

/// The `#[serde(..)]` keys of one item, container, variant or field.
#[derive(Default)]
struct Serde {
    rename: Option<String>,
    rename_all: Option<Rule>,
    rename_all_fields: Option<Rule>,
    tag: Option<String>,
    content: Option<String>,
    untagged: bool,
    transparent: bool,
    skip: bool,
    optional: bool,
    /// `default`: deserializing fills a missing field, so a caller may leave it out.
    default: bool,
    with: Option<String>,
}

/// `rename = ".."` or `rename(serialize = "..", deserialize = "..")`: the serialize name,
/// refused when the two differ (the ABI has one name for both directions).
fn one_name(meta: &syn::meta::ParseNestedMeta<'_>) -> syn::Result<LitStr> {
    if meta.input.peek(syn::Token![=]) {
        return meta.value()?.parse();
    }
    let mut ser = None;
    let mut de = None;
    meta.parse_nested_meta(|inner| {
        let lit: LitStr = inner.value()?.parse()?;
        if inner.path.is_ident("serialize") {
            ser = Some(lit);
        } else if inner.path.is_ident("deserialize") {
            de = Some(lit);
        }
        Ok(())
    })?;
    match (ser, de) {
        (Some(s), Some(d)) if s.value() != d.value() => Err(meta.error(
            "AbiType needs one name for both directions: the ABI describes calls and answers alike",
        )),
        (Some(s), _) => Ok(s),
        (None, Some(d)) => Ok(d),
        (None, None) => Err(meta.error("expected a name")),
    }
}

fn read(attrs: &[Attribute]) -> syn::Result<Serde> {
    let mut out = Serde::default();
    for attr in attrs.iter().filter(|a| a.path().is_ident("serde")) {
        attr.parse_nested_meta(|meta| {
            let key = meta
                .path
                .get_ident()
                .map(ToString::to_string)
                .unwrap_or_default();
            match key.as_str() {
                "rename" => out.rename = Some(one_name(&meta)?.value()),
                "rename_all" => out.rename_all = Some(Rule::parse(&one_name(&meta)?)?),
                "rename_all_fields" => {
                    out.rename_all_fields = Some(Rule::parse(&one_name(&meta)?)?);
                }
                "tag" => out.tag = Some(meta.value()?.parse::<LitStr>()?.value()),
                "content" => out.content = Some(meta.value()?.parse::<LitStr>()?.value()),
                "untagged" => out.untagged = true,
                "transparent" => out.transparent = true,
                "skip" | "skip_serializing" => out.skip = true,
                "skip_serializing_if" => {
                    let _: LitStr = meta.value()?.parse()?;
                    out.optional = true;
                }
                "with" | "serialize_with" => {
                    out.with = Some(meta.value()?.parse::<LitStr>()?.value());
                }
                "default" => {
                    // `default` or `default = "path"`.
                    if meta.input.peek(syn::Token![=]) {
                        let _: LitStr = meta.value()?.parse()?;
                    }
                    out.default = true;
                }
                "flatten" | "into" | "from" | "try_from" | "remote" | "getter" => {
                    return Err(meta.error(format!(
                        "AbiType cannot describe #[serde({key})]: implement `AbiType` by hand"
                    )));
                }
                _ => {
                    // Keys that do not change the JSON written (`bound`, `crate`,
                    // `deny_unknown_fields`, `alias`, `skip_deserializing`, ...): their value,
                    // if any, is skipped.
                    if meta.input.peek(syn::Token![=]) {
                        let _: syn::Expr = meta.value()?.parse()?;
                    } else if meta.input.peek(syn::token::Paren) {
                        let _ = meta.parse_nested_meta(|inner| {
                            if inner.input.peek(syn::Token![=]) {
                                let _: syn::Expr = inner.value()?.parse()?;
                            }
                            Ok(())
                        });
                    }
                }
            }
            Ok(())
        })?;
    }
    Ok(out)
}

/// `#[abi(plain)]`: the item's 64-bit integers and byte vectors travel as plain serde writes
/// them.
fn plain(attrs: &[Attribute]) -> syn::Result<bool> {
    let mut plain = false;
    for attr in attrs.iter().filter(|a| a.path().is_ident("abi")) {
        attr.parse_nested_meta(|meta| {
            if !meta.path.is_ident("plain") {
                return Err(meta.error("expected `#[abi(plain)]`"));
            }
            plain = true;
            Ok(())
        })?;
    }
    Ok(plain)
}

/// The SDK's D20 helpers (`calimero_sdk::json::*`): the JSON form the ABI type says.
const D20: &[&str] = &[
    "u64_str",
    "i64_str",
    "opt_u64_str",
    "opt_i64_str",
    "bytes_b64url",
    "opt_bytes_b64url",
    "u64_auto",
    "i64_auto",
    "opt_u64_auto",
    "opt_i64_auto",
    "bytes_auto",
    "opt_bytes_auto",
];

/// A field's ABI type, refusing what the ABI would describe wrongly. `plain`: the field is
/// `#[abi(plain)]` (or its type is).
fn field_type(
    ty: &syn::Type,
    serde: &Serde,
    plain: bool,
    span: proc_macro2::Span,
) -> syn::Result<String> {
    match &serde.with {
        Some(with) => {
            let module = with.rsplit("::").next().unwrap_or(with);
            if wire(ty).is_none() || !D20.contains(&module) {
                return Err(syn::Error::new(
                    span,
                    format!(
                        "AbiType cannot see what `with = \"{with}\"` writes; only the SDK's D20 \
                         helpers on u64, i64 and Vec<u8> (`calimero_sdk::json::u64_str`, ...) \
                         are described: implement `AbiType` by hand"
                    ),
                ));
            }
        }
        None if plain => return Ok(plain_abi_type(ty)),
        None if wire(ty).is_some() => {
            return Err(syn::Error::new(
                span,
                "plain serde writes u64/i64 as a number that loses precision and Vec<u8> as an \
                 array, where the ABI says a string (D20): add `#[serde(with = \
                 \"calimero_sdk::json::u64_str\")]` (`i64_str`, `bytes_b64url`, `opt_..`), or \
                 make it a Record; or, if the JSON is meant to be a number or an array of \
                 numbers, mark the field `#[abi(plain)]`",
            ));
        }
        None => check_wire(ty).map_err(|e| {
            syn::Error::new(
                span,
                format!(
                    "{e}; or, if the JSON is meant to be numbers, mark the field `#[abi(plain)]`"
                ),
            )
        })?,
    }
    Ok(abi_type(ty))
}

struct Ctx<'a> {
    own: &'a Ident,
    params: Vec<Ident>,
    deps: Vec<TokenStream>,
    /// The type is `#[abi(plain)]`: every field is.
    plain: bool,
    /// The struct is `#[serde(default)]`: every field may be left out.
    default: bool,
}

impl Ctx<'_> {
    fn dep(&mut self, ty: &syn::Type) {
        let own = [self.own.clone()];
        self.deps.extend(probes(ty, &own, &self.params));
    }

    /// Named fields as `[name, type]` (`[name, type, {"optional":true}]`), the skipped ones
    /// left out.
    fn fields(&mut self, fields: &syn::FieldsNamed, rule: Option<Rule>) -> syn::Result<Value> {
        let mut out = Vec::new();
        for f in &fields.named {
            let serde = read(&f.attrs)?;
            if serde.skip {
                continue;
            }
            // SAFETY: guaranteed by `FieldsNamed`, whose fields all have identifiers.
            let ident = f.ident.as_ref().expect("named field").to_string();
            let ident = ident.strip_prefix("r#").unwrap_or(&ident).to_owned();
            let name = serde
                .rename
                .clone()
                .unwrap_or_else(|| rule.map_or(ident.clone(), |r| r.field(&ident)));
            let ty = field_type(&f.ty, &serde, self.plain || plain(&f.attrs)?, f.span())?;
            self.dep(&f.ty);
            out.push(if serde.optional || serde.default || self.default {
                json!([name, ty, { "optional": true }])
            } else {
                json!([name, ty])
            });
        }
        Ok(Value::Array(out))
    }

    fn items(&mut self, fields: &syn::FieldsUnnamed) -> syn::Result<Vec<Value>> {
        fields
            .unnamed
            .iter()
            .filter_map(|f| match read(&f.attrs) {
                Ok(s) if s.skip => None,
                Ok(s) => Some(
                    plain(&f.attrs)
                        .and_then(|p| field_type(&f.ty, &s, self.plain || p, f.span()))
                        .map(|t| {
                            self.dep(&f.ty);
                            Value::String(t)
                        }),
                ),
                Err(e) => Some(Err(e)),
            })
            .collect()
    }
}

pub fn expand(input: &DeriveInput) -> syn::Result<TokenStream> {
    let sdk = crate::sdk::path();
    let name = &input.ident;
    let container = read(&input.attrs)?;
    let mut ctx = Ctx {
        own: name,
        params: input
            .generics
            .type_params()
            .map(|p| p.ident.clone())
            .collect(),
        deps: Vec::new(),
        plain: plain(&input.attrs)?,
        default: container.default,
    };
    let mut shape = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(named) if container.transparent => {
                let live: Vec<&syn::Field> = named
                    .named
                    .iter()
                    .filter(|f| read(&f.attrs).map(|s| !s.skip).unwrap_or(true))
                    .collect();
                let [field] = live.as_slice() else {
                    return Err(syn::Error::new_spanned(name, "transparent needs one field"));
                };
                let field_plain = ctx.plain || plain(&field.attrs)?;
                let ty = field_type(&field.ty, &read(&field.attrs)?, field_plain, field.span())?;
                ctx.dep(&field.ty);
                json!({ "kind": "newtype", "type": ty })
            }
            Fields::Named(named) => {
                json!({ "kind": "record", "fields": ctx.fields(named, container.rename_all)? })
            }
            Fields::Unnamed(unnamed) => {
                let items = ctx.items(unnamed)?;
                match <[Value; 1]>::try_from(items) {
                    Ok([one]) => json!({ "kind": "newtype", "type": one }),
                    Err(items) => json!({ "kind": "tuple", "items": items }),
                }
            }
            Fields::Unit => json!({ "kind": "unit" }),
        },
        Data::Enum(data) => {
            let mut variants = Vec::new();
            for v in &data.variants {
                let serde = read(&v.attrs)?;
                if serde.skip {
                    continue;
                }
                let vname = serde.rename.clone().unwrap_or_else(|| {
                    let n = v.ident.to_string();
                    container.rename_all.map_or(n.clone(), |r| r.variant(&n))
                });
                let rule = serde.rename_all.or(container.rename_all_fields);
                variants.push(match &v.fields {
                    Fields::Unit => json!({ "name": vname }),
                    Fields::Named(named) => {
                        json!({ "name": vname, "fields": ctx.fields(named, rule)? })
                    }
                    Fields::Unnamed(unnamed) => {
                        let items = ctx.items(unnamed)?;
                        match <[Value; 1]>::try_from(items) {
                            Ok([one]) => json!({ "name": vname, "type": one }),
                            Err(items) => json!({ "name": vname, "items": items }),
                        }
                    }
                });
            }
            let mut shape = json!({ "kind": "enum", "variants": variants });
            if container.untagged {
                shape["untagged"] = json!(true);
            } else if let Some(tag) = &container.tag {
                shape["tag"] = json!(tag);
                if let Some(content) = &container.content {
                    shape["content"] = json!(content);
                }
            }
            shape
        }
        Data::Union(u) => {
            return Err(syn::Error::new_spanned(
                u.union_token,
                "AbiType describes structs and enums",
            ))
        }
    };
    if !ctx.params.is_empty() {
        let params: Vec<String> = ctx.params.iter().map(ToString::to_string).collect();
        shape["params"] = json!(params);
    }
    let shape = canonical(&shape);
    let deps = probe_slice(&ctx.deps);
    let key = name.to_string();
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics #sdk::abi::AbiType for #name #ty_generics #where_clause {
            const DEF: &'static #sdk::abi::TypeDef = &#sdk::abi::TypeDef {
                name: #key,
                shape: #shape,
                deps: #deps,
            };
        }
    })
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    fn shape(input: DeriveInput) -> String {
        let out = expand(&input).unwrap().to_string();
        let start = out.find("shape :").unwrap() + "shape :".len();
        let end = start + out[start..].find(", deps :").unwrap();
        let lit: syn::LitStr = syn::parse_str(out[start..end].trim()).unwrap();
        lit.value()
    }

    #[test]
    fn rename_rules_follow_serde() {
        assert_eq!(Rule::Snake.variant("NotFound"), "not_found");
        assert_eq!(Rule::ScreamingKebab.variant("NotFound"), "NOT-FOUND");
        assert_eq!(Rule::Camel.variant("NotFound"), "notFound");
        assert_eq!(Rule::Camel.field("room_id"), "roomId");
        assert_eq!(Rule::Pascal.field("room_id"), "RoomId");
        assert_eq!(Rule::Kebab.field("room_id"), "room-id");
    }

    #[test]
    fn a_struct_with_serde_renames() {
        let s = shape(parse_quote! {
            #[serde(rename_all = "camelCase")]
            struct Item {
                room_id: String,
                #[serde(rename = "n")]
                count: u32,
                #[serde(with = "calimero_sdk::json::u64_str")]
                at: u64,
                #[serde(skip_serializing_if = "Option::is_none")]
                note: Option<String>,
                #[serde(skip)]
                cache: Vec<u32>,
            }
        });
        assert_eq!(
            s,
            r#"{"fields":[["roomId","string"],["n","u32"],["at","u64"],["note","option<string>",{"optional":true}]],"kind":"record"}"#
        );
    }

    #[test]
    fn enums_keep_serde_s_representation() {
        let s = shape(parse_quote! {
            #[serde(tag = "type", rename_all = "snake_case")]
            enum Status { Open, ClosedAt { at: u32 } }
        });
        assert_eq!(
            s,
            r#"{"kind":"enum","tag":"type","variants":[{"name":"open"},{"fields":[["at","u32"]],"name":"closed_at"}]}"#
        );
        let s = shape(parse_quote! { enum E { A, B(String), C(u8, u8) } });
        assert_eq!(
            s,
            r#"{"kind":"enum","variants":[{"name":"A"},{"name":"B","type":"string"},{"items":["u8","u8"],"name":"C"}]}"#
        );
    }

    #[test]
    fn a_generic_struct_names_its_parameters() {
        let s = shape(parse_quote! { struct Page<T> { items: Vec<T>, next: Option<String> } });
        assert_eq!(
            s,
            r#"{"fields":[["items","list<T>"],["next","option<string>"]],"kind":"record","params":["T"]}"#
        );
    }

    #[test]
    fn plain_fields_are_json_numbers_and_byte_lists() {
        let s = shape(parse_quote! {
            #[serde(rename_all = "camelCase")]
            struct Stats {
                #[abi(plain)]
                views: u64,
                #[abi(plain)]
                deltas: Vec<i64>,
                #[abi(plain)]
                shots: Vec<u8>,
                #[abi(plain)]
                peak: Option<u64>,
                #[serde(with = "calimero_sdk::json::u64_str")]
                at: u64,
            }
        });
        assert_eq!(
            s,
            r#"{"fields":[["views","u64_number"],["deltas","list<i64_number>"],["shots","byte_list"],["peak","option<u64_number>"],["at","u64"]],"kind":"record"}"#
        );
        // On the type: every field, a D20 `with` still winning; tuple fields too.
        let s = shape(parse_quote! {
            #[abi(plain)]
            struct Plain {
                size: u64,
                #[serde(with = "calimero_sdk::json::bytes_b64url")]
                blob: Vec<u8>,
            }
        });
        assert_eq!(
            s,
            r#"{"fields":[["size","u64_number"],["blob","bytes"]],"kind":"record"}"#
        );
        let s = shape(parse_quote! { struct Pair(#[abi(plain)] i64, #[abi(plain)] Vec<u8>); });
        assert_eq!(s, r#"{"items":["i64_number","byte_list"],"kind":"tuple"}"#);
        let s = shape(parse_quote! { #[abi(plain)] enum Shot { At { x: u64 } } });
        assert_eq!(
            s,
            r#"{"kind":"enum","variants":[{"fields":[["x","u64_number"]],"name":"At"}]}"#
        );
        let input: DeriveInput = parse_quote! { struct A { #[abi(number)] at: u64 } };
        assert!(expand(&input).is_err());
    }

    #[test]
    fn default_fields_are_optional() {
        let s = shape(parse_quote! {
            struct Change {
                insert: String,
                #[serde(default)]
                attributes: Option<String>,
                #[serde(default = "zero")]
                retain: u32,
            }
        });
        assert_eq!(
            s,
            r#"{"fields":[["insert","string"],["attributes","option<string>",{"optional":true}],["retain","u32",{"optional":true}]],"kind":"record"}"#
        );
        let s = shape(parse_quote! {
            #[serde(default, rename_all = "camelCase")]
            struct UpdateCalendarEvent { title: Option<String>, start_at: Option<String> }
        });
        assert_eq!(
            s,
            r#"{"fields":[["title","option<string>",{"optional":true}],["startAt","option<string>",{"optional":true}]],"kind":"record"}"#
        );
    }

    #[test]
    fn what_the_abi_would_describe_wrongly_is_refused() {
        for input in [
            parse_quote! { struct A { at: u64 } },
            parse_quote! { struct A { data: Vec<u8> } },
            parse_quote! { struct A { xs: Vec<u64> } },
            parse_quote! { struct A { #[serde(flatten)] inner: B } },
            parse_quote! { struct A { #[serde(with = "my::fmt")] at: String } },
        ] {
            let input: DeriveInput = input;
            assert!(expand(&input).is_err(), "{}", quote!(#input));
        }
    }
}
