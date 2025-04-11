use std::fmt;

use calimero_primitives::alias::Alias;
use owo_colors::OwoColorize;

pub fn pretty_alias<T: fmt::Display>(alias: Option<Alias<T>>, value: &T) -> String {
    let Some(alias) = alias else {
        return value.cyan().to_string();
    };

    format!("{} ({})", alias.cyan(), value.cyan())
}
