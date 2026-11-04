//! `RichText`: a [`Text`] plus formatting marks, Peritext-style (v1's `RichText`).
//!
//! A mark is a row `(name, "k", id)` beside the text's characters, written once:
//! `{key, value, start, end, endInclusive}`. It covers the characters from `start` to `end`
//! in document order, deleted ones included, so it keeps covering the same characters
//! however the text changes around them. Removing formatting is a newer mark with
//! `value: null`. Reading is one rule: per character and key, the covering mark with the
//! greatest id wins.
//!
//! Whether a mark grows when text is typed at its end is fixed when it is written: an
//! expanding mark (bold) ends just before the character that followed its last one
//! (`endInclusive: false`, `end: null` at the end of the text), so text typed right after
//! its last character is inside; a non-expanding mark (a link) ends at its last character
//! (`endInclusive: true`). Text typed before a mark's first character is never inside.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::app::StateField;
use crate::collections::text::{Text, Tree};
use crate::commons::{bounds, clear_prefix, collection_key, key_after};
use crate::encoding::key::KeyPart;
use crate::encoding::value;
use crate::error::StorageError;
use crate::store::{DefaultStore, RangeCursor, Store, MAX_RANGE_BATCH};

/// One formatting mark as stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mark {
    /// The attribute, e.g. `"bold"` or `"link"`.
    pub key: String,
    /// Its value; `None` removes the attribute from the covered characters.
    pub value: Option<String>,
    /// The id of the first covered character.
    pub start: u64,
    /// The id of the last covered character (`end_inclusive`), or of the character after
    /// the last one; `None` runs to the end of the text.
    pub end: Option<u64>,
    /// Whether `end` is itself covered.
    pub end_inclusive: bool,
}

/// A run of characters with the same attributes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    /// The characters.
    pub text: String,
    /// Their attributes, by key.
    pub attrs: BTreeMap<String, String>,
}

/// A Fugue text with formatting marks; see the module docs.
pub struct RichText<S: Store = DefaultStore> {
    text: Text<S>,
}

impl<S: Store> StateField for RichText<S> {
    const KIND: &'static str = "richText";
}

impl<S: Store + Default> RichText<S> {
    /// A rich text on the default store.
    pub fn new(name: impl Into<String>) -> Self {
        Self::with_store(name, S::default())
    }
}

impl<S: Store> RichText<S> {
    /// A rich text on a given store.
    pub fn with_store(name: impl Into<String>, store: S) -> Self {
        Self {
            text: Text::with_store(name, store),
        }
    }

    /// The characters, to read and edit.
    pub fn text(&self) -> &Text<S> {
        &self.text
    }

    /// The characters, to edit.
    pub fn text_mut(&mut self) -> &mut Text<S> {
        &mut self.text
    }

    fn mark_key(&self, id: u64) -> Result<Vec<u8>, StorageError> {
        collection_key(&self.text.name, "k", &[KeyPart::Int(id.into())])
    }

    /// Format the live characters `[start, end)` by index; returns the mark's id.
    /// `OutOfBounds` when the range is empty or past the end.
    pub fn mark(
        &mut self,
        start: u64,
        end: u64,
        key: &str,
        value: Option<&str>,
        expand: bool,
    ) -> Result<u64, StorageError> {
        let tree = self.text.load()?;
        let live: Vec<u64> = tree
            .order()
            .into_iter()
            .filter(|id| tree.is_live(*id))
            .collect();
        let len = live.len() as u64;
        let (Some(first), Some(last)) = (
            usize::try_from(start).ok().and_then(|i| live.get(i)),
            end.checked_sub(1)
                .filter(|_| end > start)
                .and_then(|e| usize::try_from(e).ok())
                .and_then(|i| live.get(i)),
        ) else {
            return Err(StorageError::OutOfBounds { index: end, len });
        };
        self.write_mark(&tree, *first, *last, key, value, expand)
    }

    /// Format the characters from `first` to `last` by id (either order, deleted ones
    /// included); returns the mark's id. `UnknownId` when either never existed.
    pub fn mark_ids(
        &mut self,
        first: u64,
        last: u64,
        key: &str,
        value: Option<&str>,
        expand: bool,
    ) -> Result<u64, StorageError> {
        let tree = self.text.load()?;
        self.write_mark(&tree, first, last, key, value, expand)
    }

    fn write_mark(
        &mut self,
        tree: &Tree,
        first: u64,
        last: u64,
        key: &str,
        value: Option<&str>,
        expand: bool,
    ) -> Result<u64, StorageError> {
        let order = tree.order();
        let at = |id: u64| {
            order
                .iter()
                .position(|n| *n == id)
                .ok_or(StorageError::UnknownId(id))
        };
        let (mut a, mut b) = (at(first)?, at(last)?);
        if a > b {
            core::mem::swap(&mut a, &mut b);
        }
        let (end, end_inclusive) = if expand {
            (order.get(b + 1).copied(), false)
        } else {
            (Some(order[b]), true)
        };
        let mark = Mark {
            key: key.to_owned(),
            value: value.map(str::to_owned),
            start: order[a],
            end,
            end_inclusive,
        };
        let id = self.text.mint(1)?;
        let mark_key = self.mark_key(id)?;
        self.text.store.put(&mark_key, &value::to_vec(&mark)?);
        Ok(id)
    }

    /// Every mark with its id, oldest first, read a batch at a time.
    pub fn marks(&self) -> Result<Vec<(u64, Mark)>, StorageError> {
        let base = collection_key(&self.text.name, "k", &[])?;
        let (start, end) = bounds(&base);
        let mut cursor = self.text.store.range(&start, &end, false);
        let mut out = Vec::new();
        loop {
            let batch = cursor.next(MAX_RANGE_BATCH);
            if batch.is_empty() {
                return Ok(out);
            }
            for (k, v) in batch {
                let id = match key_after(&k, &base)?.as_slice() {
                    [KeyPart::Int(id)] => u64::try_from(*id)
                        .map_err(|_| StorageError::Decode("a negative mark id".into()))?,
                    _ => return Err(StorageError::Decode("not a mark key".into())),
                };
                out.push((id, value::from_slice(&v)?));
            }
        }
    }

    /// The text as runs of equally formatted characters, in order.
    pub fn spans(&self) -> Result<Vec<Span>, StorageError> {
        let tree = self.text.load()?;
        let order = tree.order();
        let position: BTreeMap<u64, usize> =
            order.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        // Each mark as covered positions `[from, to)` of `order`, oldest first.
        let ranges: Vec<(usize, usize, Mark)> = self
            .marks()?
            .into_iter()
            .filter_map(|(_, mark)| {
                let from = *position.get(&mark.start)?;
                let to = match (mark.end, mark.end_inclusive) {
                    (None, _) => order.len(),
                    (Some(end), inclusive) => *position.get(&end)? + usize::from(inclusive),
                };
                Some((from, to, mark))
            })
            .collect();
        let mut spans: Vec<Span> = Vec::new();
        for (at, id) in order.iter().enumerate() {
            let Some(ch) = tree.nodes.get(id).and_then(|n| n.ch) else {
                continue;
            };
            let mut attrs = BTreeMap::new();
            for (_, _, mark) in ranges.iter().filter(|(f, t, _)| *f <= at && at < *t) {
                match &mark.value {
                    Some(v) => {
                        let _old = attrs.insert(mark.key.clone(), v.clone());
                    }
                    None => {
                        let _old = attrs.remove(&mark.key);
                    }
                }
            }
            match spans.last_mut() {
                Some(span) if span.attrs == attrs => span.text.push(ch),
                _ => spans.push(Span {
                    text: ch.to_string(),
                    attrs,
                }),
            }
        }
        Ok(spans)
    }

    /// Remove every character and mark.
    pub fn clear(&mut self) -> Result<(), StorageError> {
        let base = collection_key(&self.text.name, "k", &[])?;
        let _removed = clear_prefix(&mut self.text.store, &base);
        self.text.clear()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MemStore;

    fn doc() -> RichText<MemStore> {
        RichText::with_store("doc", MemStore::new())
    }

    fn render(spans: &[Span]) -> String {
        spans
            .iter()
            .map(|s| {
                if s.attrs.is_empty() {
                    s.text.clone()
                } else {
                    let attrs: Vec<String> =
                        s.attrs.iter().map(|(k, v)| format!("{k}={v}")).collect();
                    format!("[{}|{}]", s.text, attrs.join(","))
                }
            })
            .collect()
    }

    fn bold() -> Option<&'static str> {
        Some("true")
    }

    #[test]
    fn marks_format_ranges_and_the_newest_wins() {
        let mut d = doc();
        let _ = d.text_mut().insert(0, "hello world").unwrap();
        let _ = d.mark(0, 5, "bold", bold(), true).unwrap();
        assert_eq!(render(&d.spans().unwrap()), "[hello|bold=true] world");
        let _ = d.mark(2, 8, "italic", Some("1"), true).unwrap();
        assert_eq!(
            render(&d.spans().unwrap()),
            "[he|bold=true][llo|bold=true,italic=1][ wo|italic=1]rld"
        );
        let _ = d.mark(0, 3, "bold", None, true).unwrap();
        assert_eq!(
            render(&d.spans().unwrap()),
            "he[l|italic=1][lo|bold=true,italic=1][ wo|italic=1]rld"
        );
        assert_eq!(d.marks().unwrap().len(), 3);
    }

    #[test]
    fn an_expanding_mark_grows_at_its_end_and_a_link_does_not() {
        let mut d = doc();
        let ids = d.text_mut().insert(0, "ab cd").unwrap();
        let _ = d.mark(0, 2, "bold", bold(), true).unwrap();
        let _ = d.mark(3, 5, "link", Some("x"), false).unwrap();
        // Typed right after 'b' and right after 'd'.
        let _ = d.text_mut().insert_after(Some(ids[1]), "B").unwrap();
        let _ = d.text_mut().insert_after(Some(ids[4]), "D").unwrap();
        // Typed right before 'a': never inside.
        let _ = d.text_mut().insert(0, ">").unwrap();
        assert_eq!(render(&d.spans().unwrap()), ">[abB|bold=true] [cd|link=x]D");
    }

    #[test]
    fn a_mark_to_the_end_of_the_text_covers_what_is_appended() {
        let mut d = doc();
        let _ = d.text_mut().insert(0, "ab").unwrap();
        let _ = d.mark(0, 2, "bold", bold(), true).unwrap();
        let _ = d.text_mut().insert(2, "c").unwrap();
        assert_eq!(render(&d.spans().unwrap()), "[abc|bold=true]");
    }

    #[test]
    fn marks_survive_deletes_inside_and_at_their_edges() {
        let mut d = doc();
        let _ = d.text_mut().insert(0, "abcdef").unwrap();
        let _ = d.mark(1, 5, "bold", bold(), true).unwrap();
        let _ = d.text_mut().delete(1, 1).unwrap();
        let _ = d.text_mut().delete(2, 1).unwrap();
        assert_eq!(render(&d.spans().unwrap()), "a[ce|bold=true]f");
    }

    #[test]
    fn mark_by_ids_and_bad_ranges() {
        let mut d = doc();
        let ids = d.text_mut().insert(0, "abc").unwrap();
        let _ = d.mark_ids(ids[2], ids[1], "u", Some("1"), false).unwrap();
        assert_eq!(render(&d.spans().unwrap()), "a[bc|u=1]");
        assert_eq!(
            d.mark_ids(ids[0], 99, "u", None, false).unwrap_err(),
            StorageError::UnknownId(99)
        );
        assert!(matches!(
            d.mark(2, 2, "u", None, false),
            Err(StorageError::OutOfBounds { .. })
        ));
        assert!(matches!(
            d.mark(0, 4, "u", None, false),
            Err(StorageError::OutOfBounds { .. })
        ));
    }

    #[test]
    fn clear_removes_text_and_marks() {
        let store = MemStore::new();
        let mut d: RichText<MemStore> = RichText::with_store("doc", store.clone());
        let _ = d.text_mut().insert(0, "abc").unwrap();
        let _ = d.mark(0, 3, "bold", bold(), true).unwrap();
        d.clear().unwrap();
        assert!(d.spans().unwrap().is_empty());
        assert!(d.marks().unwrap().is_empty());
        assert_eq!(store.len(), 1, "only the next id stays");
    }
}
