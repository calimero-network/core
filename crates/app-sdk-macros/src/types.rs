//! Names and types as the ABI writes them: camelCase names, ABI type strings, and the
//! 64-bit integer fields that travel as decimal strings in JSON (D20).

use quote::ToTokens;
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

/// A 64-bit integer, plain or optional: JSON writes it as a decimal string (D20).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Int64 {
    U64,
    I64,
    OptU64,
    OptI64,
}

impl Int64 {
    /// The `calimero_app_sdk::json` module that reads and writes this field in JSON only.
    pub fn json_module(self) -> &'static str {
        match self {
            Self::U64 => "::calimero_app_sdk::json::u64_str",
            Self::I64 => "::calimero_app_sdk::json::i64_str",
            Self::OptU64 => "::calimero_app_sdk::json::opt_u64_str",
            Self::OptI64 => "::calimero_app_sdk::json::opt_i64_str",
        }
    }

    /// The module that writes a string in human-readable formats (JSON) and an integer
    /// otherwise (CBOR), for record fields.
    pub fn auto_module(self) -> &'static str {
        match self {
            Self::U64 => "::calimero_app_sdk::json::u64_auto",
            Self::I64 => "::calimero_app_sdk::json::i64_auto",
            Self::OptU64 => "::calimero_app_sdk::json::opt_u64_auto",
            Self::OptI64 => "::calimero_app_sdk::json::opt_i64_auto",
        }
    }

    pub fn is_option(self) -> bool {
        matches!(self, Self::OptU64 | Self::OptI64)
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

/// Whether `ty` is `u64`, `i64` or an `Option` of one.
pub fn int64(ty: &Type) -> Option<Int64> {
    match ident_of(ty).as_deref() {
        Some("u64") => return Some(Int64::U64),
        Some("i64") => return Some(Int64::I64),
        _ => {}
    }
    match ident_of(generic_arg(ty, "Option")?).as_deref() {
        Some("u64") => Some(Int64::OptU64),
        Some("i64") => Some(Int64::OptI64),
        _ => None,
    }
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
    fn int64_fields() {
        assert_eq!(int64(&parse_quote!(u64)), Some(Int64::U64));
        assert_eq!(int64(&parse_quote!(Option<i64>)), Some(Int64::OptI64));
        assert_eq!(int64(&parse_quote!(u32)), None);
        assert_eq!(int64(&parse_quote!(Vec<u64>)), None);
    }
}
