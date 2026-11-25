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
use crate::collections::text::Text;
use crate::commons::{bounds, clear_prefix, collection_key, key_after, KeyPath};
use crate::encoding::key::KeyPart;
use crate::encoding::value;
use crate::error::StorageError;
use crate::store::{DefaultStore, RangeCursor, Store, MAX_RANGE_BATCH};
use crate::AbiType;

/// One formatting mark as stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, AbiType)]
#[serde(rename_all = "camelCase")]
pub struct Mark {
    /// The attribute, e.g. `"bold"` or `"link"`.
    pub key: String,
    /// Its value; `None` removes the attribute from the covered characters.
    pub value: Option<String>,
    /// The id of the first covered character: an integer when stored, a decimal string in
    /// JSON (D20).
    #[serde(with = "crate::json::u64_auto")]
    pub start: u64,
    /// The id of the last covered character (`end_inclusive`), or of the character after
    /// the last one; `None` runs to the end of the text.
    #[serde(default, with = "crate::json::opt_u64_auto")]
    pub end: Option<u64>,
    /// Whether `end` is itself covered.
    pub end_inclusive: bool,
}

/// A run of characters with the same attributes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, AbiType)]
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
        let len = self.text.len()?;
        if start >= end || end > len {
            return Err(StorageError::OutOfBounds { index: end, len });
        }
        let (Some(first), Some(last)) = (self.text.id_at(start)?, self.text.id_at(end - 1)?) else {
            return Err(StorageError::OutOfBounds { index: end, len });
        };
        self.write_mark(first, last, key, value, expand)
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
        self.write_mark(first, last, key, value, expand)
    }

    fn write_mark(
        &mut self,
        first: u64,
        last: u64,
        key: &str,
        value: Option<&str>,
        expand: bool,
    ) -> Result<u64, StorageError> {
        let (start, last, after) = self.text.ordered(first, last)?;
        let (end, end_inclusive) = if expand {
            (after, false)
        } else {
            (Some(last), true)
        };
        let mark = Mark {
            key: key.to_owned(),
            value: value.map(str::to_owned),
            start,
            end,
            end_inclusive,
        };
        let id = self.text.mint(1)?;
        let mark_key = self.mark_key(id)?;
        self.text.store.put(&mark_key, &value::to_vec(&mark)?)?;
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

    /// The text as runs of equally formatted characters, in order: one pass over the
    /// characters with the marks opened and closed where they start and end, O(characters +
    /// marks log marks).
    pub fn spans(&self) -> Result<Vec<Span>, StorageError> {
        let runs = self.text.sequence()?;
        // Each run's start, with its first position in document order (tombstones count).
        let mut at = BTreeMap::new();
        let mut total = 0;
        for (start, run) in &runs {
            let _old = at.insert(*start, (total, run.len()));
            total += run.len();
        }
        let position = |id: u64| {
            let (start, (base, len)) = at.range(..=id).next_back()?;
            (id - start < *len).then_some(base + (id - start))
        };
        // Each mark as covered positions `[from, to)`, oldest first.
        let marks: Vec<(u64, u64, Mark)> = self
            .marks()?
            .into_iter()
            .filter_map(|(_, mark)| {
                let from = position(mark.start)?;
                let to = match mark.end {
                    None => total,
                    Some(end) => position(end)? + u64::from(mark.end_inclusive),
                };
                (from < to).then_some((from, to, mark))
            })
            .collect();
        let mut opens: Vec<(u64, usize)> =
            marks.iter().enumerate().map(|(i, m)| (m.0, i)).collect();
        let mut closes: Vec<(u64, usize)> =
            marks.iter().enumerate().map(|(i, m)| (m.1, i)).collect();
        opens.sort_unstable();
        closes.sort_unstable();
        let (mut opens, mut closes) = (opens.into_iter().peekable(), closes.into_iter().peekable());
        // Per key, the covering marks by age: the newest decides.
        let mut active: BTreeMap<&str, BTreeMap<usize, Option<&String>>> = BTreeMap::new();
        let mut spans: Vec<Span> = Vec::new();
        let mut changed = true;
        let mut pos = 0;
        for (_, run) in &runs {
            for cell in &run.cells {
                while let Some((_, i)) = opens.next_if(|(from, _)| *from <= pos) {
                    let mark = &marks[i].2;
                    let _old = active
                        .entry(mark.key.as_str())
                        .or_default()
                        .insert(i, mark.value.as_ref());
                    changed = true;
                }
                while let Some((_, i)) = closes.next_if(|(to, _)| *to <= pos) {
                    let key = marks[i].2.key.as_str();
                    if let Some(by_age) = active.get_mut(key) {
                        let _old = by_age.remove(&i);
                        if by_age.is_empty() {
                            let _gone = active.remove(key);
                        }
                    }
                    changed = true;
                }
                pos += 1;
                let Some(ch) = cell else {
                    continue;
                };
                if changed || spans.is_empty() {
                    changed = false;
                    let attrs: BTreeMap<String, String> = active
                        .iter()
                        .filter_map(|(key, by_age)| {
                            let (_, value) = by_age.last_key_value()?;
                            Some(((*key).to_owned(), (*value)?.clone()))
                        })
                        .collect();
                    if spans.last().is_none_or(|span| span.attrs != attrs) {
                        spans.push(Span {
                            text: String::new(),
                            attrs,
                        });
                    }
                }
                if let Some(span) = spans.last_mut() {
                    span.text.push(*ch);
                }
            }
        }
        Ok(spans)
    }

    /// Remove every character and mark.
    pub fn clear(&mut self) -> Result<(), StorageError> {
        let base = collection_key(&self.text.name, "k", &[])?;
        let _removed = clear_prefix(&mut self.text.store, &base)?;
        self.text.clear()
    }
}

impl<S: Store> crate::collections::Nestable<S> for RichText<S> {
    fn at(path: KeyPath, store: S) -> Self {
        Self {
            text: <Text<S> as crate::collections::Nestable<S>>::at(path, store),
        }
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

    #[test]
    fn marks_hold_across_run_splits() {
        let mut d = doc();
        // One run; the bold mark ends inside it, before the space.
        let ids = d.text_mut().insert(0, "hello world").unwrap();
        let _ = d.mark(0, 5, "bold", bold(), true).unwrap();
        let _ = d
            .mark_ids(ids[8], ids[6], "link", Some("x"), false)
            .unwrap();
        // Split the run inside each mark and at the bold mark's end.
        let _ = d.text_mut().insert_after(Some(ids[1]), "E").unwrap();
        let _ = d.text_mut().insert_after(Some(ids[7]), "O").unwrap();
        let _ = d.text_mut().insert_after(Some(ids[4]), "!").unwrap();
        assert_eq!(
            render(&d.spans().unwrap()),
            "[heEllo!|bold=true] [woOr|link=x]ld"
        );
        // A mark by ids across the split pieces, then a delete across them.
        let _ = d.mark_ids(ids[2], ids[9], "u", Some("1"), false).unwrap();
        let _ = d.text_mut().delete(3, 3).unwrap();
        assert_eq!(
            render(&d.spans().unwrap()),
            "[heE|bold=true][!|bold=true,u=1][ |u=1][woOr|link=x,u=1][l|u=1]d"
        );
    }
}
