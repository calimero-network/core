//! A component app's table records as search documents: which stored keys are records, the
//! schema of a table's index and the document of a record. The node
//! (`calimero-node`'s `component::search`) and the SDK's test harness both index through
//! these, so a harness test searches what a node would; [`TableSearch`] is the whole index of
//! one table over one state, which the harness builds per view.
//!
//! A table's records are the stored keys `generation ‖ (table, "r", key..)` in the SDK's key
//! encoding (`crates/sdk`, encoding contract); each becomes one document, its id the SHA-256
//! of the stored key, its fields the declared ones that hold text (a list of strings is
//! joined), each cut at [`MAX_FIELD_TEXT_BYTES`].
//!
//! Ids in these comments cite `docs/specs/app-runtime-v0.14.md` (D decisions in section 2): D39
//! (app data is keyed by generation; host records sit outside it).

use std::collections::HashMap;

use calimero_primitives::search::{
    SearchDoc, SearchFieldKind, SearchFieldSchema, SearchIndexSchema, SearchRequest,
    SearchResponse, SearchValue,
};
use eyre::Result as EyreResult;
use sha2::{Digest, Sha256};

use crate::index::{ContextIndex, SchemaOptions};

/// Bytes of the generation prefix on a stored key (D39).
pub const GENERATION_PREFIX: usize = 8;
/// Bytes of text one document field is indexed from at most; the rest is cut.
pub const MAX_FIELD_TEXT_BYTES: usize = 64 * 1024;

/// The table a stored key is a record of: its first key part, when the second is `"r"`
/// (`(table, "r", key..)`, the SDK's record layout). `None` for any other key.
#[must_use]
pub fn record_table(stored: &[u8]) -> Option<String> {
    let key = stored.get(GENERATION_PREFIX..)?;
    let (table, rest) = string_part(key)?;
    let (kind, _) = string_part(rest)?;
    (kind == "r").then_some(table)
}

/// The key prefix of `table`'s records, without the generation prefix: `(table, "r", ..)`.
#[must_use]
pub fn record_prefix(table: &str) -> Vec<u8> {
    let mut prefix = vec![0x30];
    prefix.extend(
        table
            .bytes()
            .flat_map(|b| if b == 0 { vec![0x00, 0xFF] } else { vec![b] }),
    );
    prefix.extend_from_slice(&[0x00, 0x00, 0x30, b'r', 0x00, 0x00]);
    prefix
}

/// One string part (`0x30`, bytes with `0x00` escaped as `0x00 0xFF`, then `0x00 0x00`) and
/// what follows it.
fn string_part(key: &[u8]) -> Option<(String, &[u8])> {
    let mut rest = key.strip_prefix(&[0x30])?;
    let mut out = Vec::new();
    loop {
        match rest {
            [0x00, 0x00, tail @ ..] => return Some((String::from_utf8(out).ok()?, tail)),
            [0x00, 0xFF, tail @ ..] => {
                out.push(0x00);
                rest = tail;
            }
            [b, tail @ ..] if *b != 0x00 => {
                out.push(*b);
                rest = tail;
            }
            _ => return None,
        }
    }
}

/// The schema of `table`'s index: its declared fields, as text.
#[must_use]
pub fn table_schema(table: &str, fields: &[String]) -> SearchIndexSchema {
    SearchIndexSchema {
        name: table.to_owned(),
        version: 1,
        fields: fields
            .iter()
            .map(|f| SearchFieldSchema {
                name: f.clone(),
                kind: SearchFieldKind::Text {
                    weight: 100,
                    infix: true,
                },
            })
            .collect(),
    }
}

/// The id of the document of the record at `stored`: the SHA-256 of the stored key.
#[must_use]
pub fn doc_id(stored: &[u8]) -> [u8; 32] {
    Sha256::digest(stored).into()
}

/// The document of a record: the declared fields that hold text. `None` for a value that is
/// not a CBOR map.
#[must_use]
pub fn document(stored: &[u8], value: &[u8], fields: &[String]) -> Option<SearchDoc> {
    let ciborium::Value::Map(entries) = ciborium::from_reader(value).ok()? else {
        return None;
    };
    let mut doc = Vec::new();
    for (name, v) in &entries {
        let Some(name) = name.as_text().filter(|n| fields.iter().any(|f| f == n)) else {
            continue;
        };
        let mut text = match v {
            ciborium::Value::Text(t) => t.clone(),
            ciborium::Value::Array(items) => items
                .iter()
                .filter_map(ciborium::Value::as_text)
                .collect::<Vec<_>>()
                .join(" "),
            _ => continue,
        };
        if text.len() > MAX_FIELD_TEXT_BYTES {
            let mut cut = MAX_FIELD_TEXT_BYTES;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            text.truncate(cut);
        }
        doc.push((name.to_owned(), SearchValue::Str(text)));
    }
    Some(SearchDoc {
        id: doc_id(stored),
        fields: doc,
    })
}

/// One table's index over one state, in memory, with the stored keys of its documents.
pub struct TableSearch {
    index: ContextIndex,
    keys: HashMap<[u8; 32], Vec<u8>>,
}

impl TableSearch {
    /// Indexes `records` (stored key and value; the caller passes `table`'s, in any order)
    /// under `table`'s declared `fields`, as a node indexes them.
    ///
    /// # Errors
    /// A tantivy failure.
    pub fn build<'a>(
        table: &str,
        fields: &[String],
        records: impl IntoIterator<Item = (&'a [u8], &'a [u8])>,
    ) -> EyreResult<Self> {
        let dir = Box::new(tantivy::directory::RamDirectory::create());
        let (index, _state) =
            ContextIndex::open(dir, &table_schema(table, fields), SchemaOptions::default())?;
        let index = index.ok_or_else(|| eyre::eyre!("a new index opened stale"))?;
        let mut keys = HashMap::new();
        let docs: Vec<SearchDoc> = records
            .into_iter()
            .filter_map(|(key, value)| {
                let doc = document(key, value, fields)?;
                let _prev = keys.insert(doc.id, key.to_vec());
                Some(doc)
            })
            .collect();
        let _n = index.apply(docs.iter().map(|d| (&d.id, Some(d))))?;
        index.commit(1, [0; 32])?;
        index.close_writer()?;
        Ok(Self { index, keys })
    }

    /// Answers `req` (its `index` is ignored) from the committed index.
    ///
    /// # Errors
    /// A query the index refuses (too long, a cursor past `MAX_CURSOR`).
    pub fn search(&self, req: &SearchRequest) -> EyreResult<SearchResponse> {
        self.index.search(req)
    }

    /// The stored key of a hit's document.
    #[must_use]
    pub fn key(&self, id: &[u8; 32]) -> Option<&[u8]> {
        self.keys.get(id).map(Vec::as_slice)
    }
}
