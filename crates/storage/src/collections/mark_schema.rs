//! The per-key boundary policy a rich-text app compiles into its WASM.
//!
//! Expand is AUTHORING-TIME ONLY: it picks the two anchor biases when a mark is
//! written and is never stored, never read back, and never consulted by a read.
//! So a replica running an older schema, or one that has never heard of a key,
//! renders a mark from its stored anchors and gets the same spans as everyone
//! else. There is no schema-version divergence in the stored bytes.

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

#[cfg(test)]
mod tests {
    use super::{DefaultMarks, Expand, MarkSchema};

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

    /// An app's own table is the only thing that decides; the default is not special.
    #[test]
    fn a_custom_schema_decides_its_own_keys() {
        struct Sparse;
        impl MarkSchema for Sparse {
            fn expand(prefix: &str) -> Option<Expand> {
                (prefix == "note").then_some(Expand::Both)
            }
        }
        assert_eq!(Sparse::expand("note"), Some(Expand::Both));
        assert_eq!(Sparse::expand("bold"), None);
        assert_eq!(DefaultMarks::expand("bold"), Some(Expand::After));
        assert_eq!(DefaultMarks::expand("link"), Some(Expand::None));
        assert_eq!(DefaultMarks::expand("nonsense"), None);
    }
}
