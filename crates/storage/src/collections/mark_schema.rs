//! The per-key boundary policy a rich-text app compiles into its WASM.
//!
//! Expand is AUTHORING-TIME ONLY: it picks the two anchor biases when a mark is
//! written and is never stored, never read back, and never consulted by a read.
//! So a replica running an older schema, or one that has never heard of a key,
//! renders a mark from its stored anchors and gets the same spans as everyone
//! else. There is no schema-version divergence in the stored bytes.

use crate::collections::error::StoreError;

/// How a formatting key behaves at the edges of its range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expand {
    /// Neither edge grows. Links, code, highlights, comments.
    None,
    /// Only the start grows.
    Before,
    /// Only the end grows. Bold, italic, underline.
    After,
    /// Both edges grow.
    Both,
}

impl Expand {
    /// The expand a REMOVAL of this key uses.
    ///
    /// Peritext states the inversion for `removeMark`; Loro ships the same table.
    #[must_use]
    pub const fn inverted(self) -> Self {
        match self {
            Self::Before => Self::Before,
            Self::After => Self::After,
            Self::Both => Self::None,
            Self::None => Self::Both,
        }
    }

    pub(crate) const fn expands_before(self) -> bool {
        matches!(self, Self::Before | Self::Both)
    }

    pub(crate) const fn expands_after(self) -> bool {
        matches!(self, Self::After | Self::Both)
    }
}

/// The boundary table an app declares, looked up by the part of a key BEFORE
/// the first `:`, so `comment:alice` and `comment:bob` are one policy and two
/// independent keys.
pub trait MarkSchema: 'static {
    /// `None` rejects the key at write time; it never affects a read.
    fn expand(prefix: &str) -> Option<Expand>;
}

/// Loro's shipped defaults. `code` is `None`, not `After`, on purpose.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DefaultMarks;

impl MarkSchema for DefaultMarks {
    fn expand(prefix: &str) -> Option<Expand> {
        Some(match prefix {
            "bold" | "italic" | "underline" | "strike" => Expand::After,
            "link" | "code" | "highlight" | "comment" => Expand::None,
            _ => return None,
        })
    }
}

/// The part of `key` before its first `:`, which is what the schema is keyed on.
/// A suffix is opaque and may itself contain colons, but it must not be empty.
pub(crate) fn mark_prefix(key: &str) -> Result<&str, StoreError> {
    let prefix = key.split(':').next().unwrap_or(key);
    if prefix.is_empty() {
        return Err(invalid("mark key must not start with ':'"));
    }
    if key.len() == prefix.len() + 1 {
        return Err(invalid("mark key suffix must not be empty"));
    }
    Ok(prefix)
}

/// The expand `key` is written with, inverted when the write REMOVES the key.
pub(crate) fn expand_for<Sc: MarkSchema>(key: &str, removing: bool) -> Result<Expand, StoreError> {
    let prefix = mark_prefix(key)?;
    let expand = Sc::expand(prefix).ok_or_else(|| {
        invalid(&format!(
            "unknown mark key '{prefix}': declare it in the app's MarkSchema"
        ))
    })?;
    Ok(if removing { expand.inverted() } else { expand })
}

fn invalid(message: &str) -> StoreError {
    StoreError::StorageError(crate::interface::StorageError::InvalidData(message.into()))
}

#[cfg(test)]
mod tests {
    use super::{expand_for, mark_prefix, DefaultMarks, Expand, MarkSchema};

    #[test]
    fn inverted__is_its_own_involution() {
        for expand in [Expand::None, Expand::Before, Expand::After, Expand::Both] {
            assert_eq!(
                expand.inverted().inverted(),
                expand,
                "{expand:?} must invert back to itself"
            );
        }
    }

    #[test]
    fn inverted__keeps_a_one_sided_expand_and_swaps_the_symmetric_ones() {
        // Removing bold must keep growing at the end, which is what makes typing
        // after an unbolded run stay unbolded.
        assert_eq!(Expand::After.inverted(), Expand::After);
        assert_eq!(Expand::Before.inverted(), Expand::Before);
        assert_eq!(Expand::Both.inverted(), Expand::None);
        assert_eq!(Expand::None.inverted(), Expand::Both);
    }

    #[test]
    fn expands__reports_the_growing_edges() {
        assert!(!Expand::None.expands_before() && !Expand::None.expands_after());
        assert!(Expand::Before.expands_before() && !Expand::Before.expands_after());
        assert!(!Expand::After.expands_before() && Expand::After.expands_after());
        assert!(Expand::Both.expands_before() && Expand::Both.expands_after());
    }

    #[test]
    fn mark_prefix__splits_at_the_first_colon_only() {
        assert_eq!(mark_prefix("bold").unwrap(), "bold");
        assert_eq!(mark_prefix("comment:alice").unwrap(), "comment");
        assert_eq!(mark_prefix("comment:a:b").unwrap(), "comment");
    }

    #[test]
    fn mark_prefix__rejects_an_empty_prefix_or_suffix() {
        for key in ["", ":", ":alice"] {
            let message = mark_prefix(key).unwrap_err().to_string();
            assert!(
                message.contains("must not start with ':'"),
                "{key:?} gave {message}"
            );
        }
        let message = mark_prefix("comment:").unwrap_err().to_string();
        assert!(message.contains("suffix must not be empty"), "{message}");
    }

    #[test]
    fn expand_for__reads_the_schema_and_inverts_a_removal() {
        assert_eq!(
            expand_for::<DefaultMarks>("bold", false).unwrap(),
            Expand::After
        );
        assert_eq!(
            expand_for::<DefaultMarks>("bold", true).unwrap(),
            Expand::After
        );
        assert_eq!(
            expand_for::<DefaultMarks>("link", false).unwrap(),
            Expand::None
        );
        assert_eq!(
            expand_for::<DefaultMarks>("link", true).unwrap(),
            Expand::Both
        );
    }

    #[test]
    fn expand_for__one_policy_covers_every_suffix_of_a_prefix() {
        assert_eq!(
            expand_for::<DefaultMarks>("comment:alice", false).unwrap(),
            expand_for::<DefaultMarks>("comment:bob", false).unwrap()
        );
    }

    #[test]
    fn expand_for__rejects_a_key_the_schema_does_not_declare() {
        let message = expand_for::<DefaultMarks>("nonsense", false)
            .unwrap_err()
            .to_string();
        assert!(message.contains("unknown mark key 'nonsense'"), "{message}");
        assert!(message.contains("MarkSchema"), "{message}");
    }

    /// An app's own table is the only thing that decides; the default is not special.
    #[test]
    fn a_custom_schema_decides_its_own_keys() {
        struct Sparse;
        impl MarkSchema for Sparse {
            fn expand(prefix: &str) -> Option<Expand> {
                (prefix == "note").then_some(Expand::Both)
            }
        }
        assert_eq!(expand_for::<Sparse>("note", false).unwrap(), Expand::Both);
        assert_eq!(expand_for::<Sparse>("note", true).unwrap(), Expand::None);
        assert!(expand_for::<Sparse>("bold", false).is_err());
    }
}
