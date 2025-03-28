use std::fmt;

use calimero_primitives::alias::Alias;

pub fn pretty_alias<T: fmt::Display>(alias: Option<Alias<T>>, value: &T) -> String {
    let Some(alias) = alias else {
        return value.to_string();
    };

    format!("{alias} ({value})")
}

pub fn get_alias_or_fallback<T: ToString>(alias: Option<&Alias<T>>, fallback: T) -> String {
    alias
        .as_ref()
        .map(|a| a.to_string())
        .unwrap_or_else(|| fallback.to_string())
}
