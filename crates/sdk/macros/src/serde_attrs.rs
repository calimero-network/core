//! The `#[serde(...)]` keys that decide a type's JSON wire shape, read so the
//! ABI describes what serde emits instead of the Rust spelling.

use proc_macro2::Span;
use syn::ext::IdentExt;
use syn::meta::ParseNestedMeta;
use syn::spanned::Spanned;
use syn::{token, Attribute, Expr, LitStr, Token};

use crate::errors::ParseError;

const SHAPE_NEUTRAL: [&str; 9] = [
    "alias",
    "borrow",
    "bound",
    "crate",
    "default",
    "deny_unknown_fields",
    "expecting",
    "getter",
    "skip_serializing_if",
]; // accepted keys that never change the JSON a value serializes to
const CUSTOM_WIRE: [&str; 3] = ["with", "serialize_with", "deserialize_with"]; // hand-written (de)serializers the ABI cannot see through

/// A serde `rename_all` rule, applied exactly as serde_derive applies it.
#[derive(Clone, Copy)]
pub enum RenameRule {
    Lower,
    Upper,
    Pascal,
    Camel,
    Snake,
    ScreamingSnake,
    Kebab,
    ScreamingKebab,
}

impl RenameRule {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "lowercase" => Self::Lower,
            "UPPERCASE" => Self::Upper,
            "PascalCase" => Self::Pascal,
            "camelCase" => Self::Camel,
            "snake_case" => Self::Snake,
            "SCREAMING_SNAKE_CASE" => Self::ScreamingSnake,
            "kebab-case" => Self::Kebab,
            "SCREAMING-KEBAB-CASE" => Self::ScreamingKebab,
            _ => return None,
        })
    }

    /// A variant name, which Rust writes in PascalCase.
    pub fn apply_to_variant(self, variant: &str) -> String {
        match self {
            Self::Pascal => variant.to_owned(),
            Self::Lower => variant.to_ascii_lowercase(),
            Self::Upper => variant.to_ascii_uppercase(),
            Self::Camel => lower_first(variant),
            Self::Snake => {
                let mut snake = String::new();
                for (i, ch) in variant.char_indices() {
                    if i > 0 && ch.is_uppercase() {
                        snake.push('_');
                    }
                    snake.push(ch.to_ascii_lowercase());
                }
                snake
            }
            Self::ScreamingSnake => Self::Snake.apply_to_variant(variant).to_ascii_uppercase(),
            Self::Kebab => Self::Snake.apply_to_variant(variant).replace('_', "-"),
            Self::ScreamingKebab => Self::ScreamingSnake
                .apply_to_variant(variant)
                .replace('_', "-"),
        }
    }

    /// A field name, which Rust writes in snake_case.
    pub fn apply_to_field(self, field: &str) -> String {
        match self {
            Self::Lower | Self::Snake => field.to_owned(),
            Self::Upper | Self::ScreamingSnake => field.to_ascii_uppercase(),
            Self::Pascal => {
                let mut pascal = String::new();
                let mut capitalize = true;
                for ch in field.chars() {
                    if ch == '_' {
                        capitalize = true;
                    } else if capitalize {
                        pascal.push(ch.to_ascii_uppercase());
                        capitalize = false;
                    } else {
                        pascal.push(ch);
                    }
                }
                pascal
            }
            Self::Camel => lower_first(&Self::Pascal.apply_to_field(field)),
            Self::Kebab => field.replace('_', "-"),
            Self::ScreamingKebab => field.to_ascii_uppercase().replace('_', "-"),
        }
    }
}

#[derive(Default)]
pub struct ContainerAttrs {
    pub rename_all: Option<RenameRule>,
    pub rename_all_fields: Option<RenameRule>,
    pub tag: Option<(String, Span)>,
    pub content: Option<String>,
    pub untagged: Option<Span>,
}

#[derive(Default)]
pub struct VariantAttrs {
    pub rename: Option<String>,
    pub rename_all: Option<RenameRule>,
    pub skip: bool,
}

#[derive(Default)]
pub struct FieldAttrs {
    pub rename: Option<String>,
    pub skip: bool,
    /// The `with`-family key and its span, when a hand-written function writes the field.
    pub custom_wire: Option<(String, Span)>,
}

pub fn container(attrs: &[Attribute]) -> syn::Result<ContainerAttrs> {
    let mut out = ContainerAttrs::default();
    for_each_key(attrs, |key, meta| {
        match key {
            "rename_all" => out.rename_all = Some(rule(key, meta)?),
            "rename_all_fields" => out.rename_all_fields = Some(rule(key, meta)?),
            "tag" => out.tag = Some((string(key, meta)?, meta.path.span())),
            "content" => out.content = Some(string(key, meta)?),
            "untagged" => out.untagged = Some(meta.path.span()),
            // A container's own name never appears in its JSON.
            "rename" => skip_value(meta)?,
            _ => return Ok(false),
        }
        Ok(true)
    })?;
    Ok(out)
}

impl ContainerAttrs {
    /// The name a variant carries on the wire.
    pub fn variant_name(&self, ident: &syn::Ident, variant: &VariantAttrs) -> String {
        variant.rename.clone().unwrap_or_else(|| {
            let ident = ident.unraw().to_string();
            self.rename_all
                .map_or_else(|| ident.clone(), |rule| rule.apply_to_variant(&ident))
        })
    }
}

pub fn variant(attrs: &[Attribute]) -> syn::Result<VariantAttrs> {
    let mut out = VariantAttrs::default();
    for_each_key(attrs, |key, meta| {
        match key {
            "rename" => out.rename = Some(string(key, meta)?),
            "rename_all" => out.rename_all = Some(rule(key, meta)?),
            "skip" => out.skip = true,
            _ => return Ok(false),
        }
        Ok(true)
    })?;
    Ok(out)
}

pub fn field(attrs: &[Attribute]) -> syn::Result<FieldAttrs> {
    let mut out = FieldAttrs::default();
    for_each_key(attrs, |key, meta| {
        match key {
            "rename" => out.rename = Some(string(key, meta)?),
            "skip" => out.skip = true,
            _ if CUSTOM_WIRE.contains(&key) => {
                skip_value(meta)?;
                out.custom_wire = Some((key.to_owned(), meta.path.span()));
            }
            _ => return Ok(false),
        }
        Ok(true)
    })?;
    Ok(out)
}

/// Runs `handle` on every key of every `#[serde(...)]`. A key it declines is
/// accepted only when shape-neutral; anything else would be misdescribed.
fn for_each_key(
    attrs: &[Attribute],
    mut handle: impl FnMut(&str, &ParseNestedMeta<'_>) -> syn::Result<bool>,
) -> syn::Result<()> {
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("serde")) {
        attr.parse_nested_meta(|meta| {
            let key = meta
                .path
                .get_ident()
                .map(ToString::to_string)
                .unwrap_or_default();
            if handle(&key, &meta)? {
                return Ok(());
            }
            if SHAPE_NEUTRAL.contains(&key.as_str()) {
                return skip_value(&meta);
            }
            Err(meta.error(ParseError::UnsupportedSerdeAttr { attr: key }))
        })?;
    }
    Ok(())
}

fn skip_value(meta: &ParseNestedMeta<'_>) -> syn::Result<()> {
    if meta.input.peek(Token![=]) {
        let _: Expr = meta.value()?.parse()?;
    } else if meta.input.peek(token::Paren) {
        meta.parse_nested_meta(|nested| skip_value(&nested))?;
    }
    Ok(())
}

/// A `key = "value"` string. The `key(serialize = .., deserialize = ..)` form
/// gives the two directions different shapes, which one ABI entry cannot hold.
fn string(key: &str, meta: &ParseNestedMeta<'_>) -> syn::Result<String> {
    if meta.input.peek(token::Paren) {
        return Err(meta.error(ParseError::UnsupportedSerdeAttr {
            attr: format!("{key}(serialize, deserialize)"),
        }));
    }
    Ok(meta.value()?.parse::<LitStr>()?.value())
}

fn rule(key: &str, meta: &ParseNestedMeta<'_>) -> syn::Result<RenameRule> {
    let value = string(key, meta)?;
    RenameRule::parse(&value)
        .ok_or_else(|| meta.error(ParseError::UnknownRenameRule { rule: value }))
}

fn lower_first(name: &str) -> String {
    let mut chars = name.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_ascii_lowercase().to_string() + chars.as_str()
    })
}

#[cfg(test)]
mod tests {
    use syn::{parse_quote, Attribute};

    use super::{container, field, variant};

    #[test]
    fn a_nested_shape_neutral_value_is_skipped() {
        let renamed: Vec<Attribute> = vec![parse_quote!(#[serde(
            rename = "blobId",
            bound(serialize = "T: Clone")
        )])];
        assert_eq!(field(&renamed).unwrap().rename.as_deref(), Some("blobId"));
    }

    #[test]
    fn keys_the_abi_cannot_describe_are_rejected() {
        let field_keys: [Attribute; 4] = [
            parse_quote!(#[serde(flatten)]),
            parse_quote!(#[serde(skip_serializing)]),
            parse_quote!(#[serde(skip_deserializing)]),
            parse_quote!(#[serde(rename(serialize = "a", deserialize = "b"))]),
        ];
        for (index, attr) in field_keys.into_iter().enumerate() {
            assert!(field(&[attr]).is_err(), "field case {index}");
        }
        let container_keys: [Attribute; 3] = [
            parse_quote!(#[serde(transparent)]),
            parse_quote!(#[serde(into = "String")]),
            parse_quote!(#[serde(rename_all = "Title Case")]),
        ];
        for (index, attr) in container_keys.into_iter().enumerate() {
            assert!(container(&[attr]).is_err(), "container case {index}");
        }
    }

    #[test]
    fn variant_keys_the_abi_cannot_describe_are_rejected() {
        for attr in [
            parse_quote!(#[serde(other)]),
            parse_quote!(#[serde(untagged)]),
            parse_quote!(#[serde(rename(serialize = "a", deserialize = "b"))]),
        ] {
            let attrs: Vec<Attribute> = vec![attr];
            assert!(variant(&attrs).is_err());
        }
    }
}
