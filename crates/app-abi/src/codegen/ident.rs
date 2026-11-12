//! Identifiers for the Swift and Kotlin clients: an ABI name made a valid identifier, and the
//! names one scope hands out kept distinct.

use std::collections::BTreeSet;

/// `name` with anything but letters, digits and `_` made `_`, never starting with a digit
/// (`0` → `_0`, `a-b` → `a_b`).
pub(super) fn sanitize(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

/// `NotFound` → `notFound`: a Swift enum case.
pub(super) fn lower_first(name: &str) -> String {
    let mut chars = name.chars();
    chars
        .next()
        .map(|c| c.to_lowercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The names declared in one scope, each handed out once: a name already taken gets `_`
/// appended until it is free.
pub(super) struct Scope(BTreeSet<String>);

impl Scope {
    pub(super) fn new(reserved: &[&str]) -> Self {
        Self(reserved.iter().map(|r| (*r).to_owned()).collect())
    }

    pub(super) fn take(&mut self, base: String) -> String {
        let mut name = base;
        while !self.0.insert(name.clone()) {
            name.push('_');
        }
        name
    }

    pub(super) fn contains(&self, name: &str) -> bool {
        self.0.contains(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers() {
        assert_eq!(sanitize("0"), "_0");
        assert_eq!(sanitize("a-b"), "a_b");
        assert_eq!(sanitize("roomId"), "roomId");
        assert_eq!(lower_first("NotFound"), "notFound");
        let mut scope = Scope::new(&["events"]);
        assert_eq!(scope.take("events".into()), "events_");
        assert_eq!(scope.take("get".into()), "get");
        assert_eq!(scope.take("get".into()), "get_");
    }
}
