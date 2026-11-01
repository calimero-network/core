//! Full-text search for component apps (spec section 7, R2): the host keeps an index of
//! each table's declared text fields, derived from the app's state, and views query it
//! through the WIT `search` interface.
//!
//! What to index comes from the bundle's ABI (`state.search`: table to fields, from
//! `#[table(search = [..])]`). A table's records are the stored keys
//! `generation ‖ (table, "r", key..)` in the SDK's key encoding (`crates/app-sdk`, encoding
//! contract); each becomes one document, its id the SHA-256 of the stored key, its fields
//! the declared ones that hold text (a list of strings is joined). The indexes are
//! `calimero-search`'s `ContextIndex` in memory, as for a wasmer app (`services::Search`):
//! a view first brings them up to the state it reads, from the record keys the entries
//! since touched (`AppMachine::touched_after`, kept for `TOUCHED_RING` entries), else by a
//! full scan of the app's records. Derived data: never in the log or a snapshot, rebuilt
//! after a restart.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, PoisonError};

use calimero_app_host::{SearchHit, SearchHost, SearchMode, SearchPage, SearchQuery, GENERATION};
use calimero_primitives::search::{
    SearchDoc, SearchFieldKind, SearchFieldSchema, SearchIndexSchema, SearchMode as IndexMode,
    SearchOrder, SearchRequest, SearchValue,
};
use calimero_search::index::{ContextIndex, SchemaOptions};
use ring::digest::{digest, SHA256};
use tracing::{debug, warn};

use crate::app::AppMachine;
use crate::types::AppId;

/// Bytes of the generation prefix on a stored key (D39).
const GENERATION_PREFIX: usize = 8;
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

/// The document of a record: the declared fields that hold text.
fn document(stored: &[u8], value: &[u8], fields: &[String]) -> Option<SearchDoc> {
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

fn doc_id(stored: &[u8]) -> [u8; 32] {
    let mut out = [0; 32];
    out.copy_from_slice(digest(&SHA256, stored).as_ref());
    out
}

/// Records to index: stored key, and the value (`None`: gone).
type Records = Vec<(Vec<u8>, Option<Vec<u8>>)>;

/// One table's index and the stored keys of its documents.
struct TableIndex {
    fields: Vec<String>,
    index: ContextIndex,
    keys: HashMap<[u8; 32], Vec<u8>>,
}

impl TableIndex {
    fn new(table: &str, fields: &[String]) -> Result<Self, String> {
        let def = SearchIndexSchema {
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
        };
        let dir = Box::new(tantivy::directory::RamDirectory::create());
        let (index, _state) =
            ContextIndex::open(dir, &def, SchemaOptions::default()).map_err(|e| e.to_string())?;
        Ok(Self {
            fields: fields.to_vec(),
            index: index.ok_or("a new index opened stale")?,
            keys: HashMap::new(),
        })
    }

    /// Replace the documents of `records` (stored key, value or `None` when gone).
    fn apply(&mut self, records: &[(Vec<u8>, Option<Vec<u8>>)], at: u64) -> Result<(), String> {
        let mut changes = Vec::with_capacity(records.len());
        for (key, value) in records {
            let id = doc_id(key);
            let doc = value
                .as_deref()
                .and_then(|v| document(key, v, &self.fields));
            if doc.is_some() {
                let _prev = self.keys.insert(id, key.clone());
            } else {
                let _gone = self.keys.remove(&id);
            }
            changes.push((id, doc));
        }
        let _n = self
            .index
            .apply(changes.iter().map(|(id, doc)| (id, doc.as_ref())))
            .map_err(|e| e.to_string())?;
        self.index.commit(at, [0; 32]).map_err(|e| e.to_string())
    }
}

/// An app's indexes, as of `built_at` on bundle `bundle`.
#[derive(Default)]
struct AppIndexes {
    bundle: String,
    built_at: u64,
    tables: HashMap<String, TableIndex>,
}

/// Full-text search over the component apps hosted here, in memory.
#[derive(Default)]
pub struct ComponentSearch {
    apps: Mutex<HashMap<AppId, Arc<Mutex<AppIndexes>>>>,
}

/// A view's search, bound to its app's indexes.
pub struct RunSearch(Arc<Mutex<AppIndexes>>);

impl SearchHost for RunSearch {
    fn query(&self, q: &SearchQuery) -> Result<SearchPage, String> {
        let ix = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(table) = ix.tables.get(&q.table) else {
            return Err(format!("table {:?} declares no search fields", q.table));
        };
        let request = SearchRequest {
            index: q.table.clone(),
            query: q.text.clone(),
            mode: match q.mode {
                SearchMode::Words => IndexMode::Words,
                SearchMode::Prefix => IndexMode::Prefix,
                SearchMode::Substring => IndexMode::Substring,
                SearchMode::Fuzzy => IndexMode::Fuzzy,
            },
            filters: Vec::new(),
            order: SearchOrder::Relevance,
            cursor: q.cursor,
            limit: q.limit,
        };
        let response = table.index.search(&request).map_err(|e| e.to_string())?;
        Ok(SearchPage {
            hits: response
                .hits
                .into_iter()
                .filter_map(|h| {
                    let key = table.keys.get(&h.id)?;
                    Some(SearchHit {
                        key: key.get(GENERATION_PREFIX..)?.to_vec(),
                        score: h.score,
                        snippet: h.snippet,
                    })
                })
                .collect(),
            next: response.next_cursor,
            total: response.total,
        })
    }
}

impl ComponentSearch {
    /// The app's search, brought up to the state `m` holds at `applied`, for a view of
    /// `bundle`, whose ABI declares `fields` (table to text fields). `None` when the app
    /// declares none, or the index could not be built (the view then gets the WIT error).
    pub fn for_view(
        &self,
        bundle: &str,
        fields: &BTreeMap<String, Vec<String>>,
        m: &AppMachine,
        applied: u64,
    ) -> Option<Arc<dyn SearchHost>> {
        if fields.is_empty() {
            return None;
        }
        let entry = {
            let mut apps = self.apps.lock().unwrap_or_else(PoisonError::into_inner);
            Arc::clone(apps.entry(m.app_id.clone()).or_default())
        };
        {
            let mut ix = entry.lock().unwrap_or_else(PoisonError::into_inner);
            if let Err(e) = refresh(&mut ix, bundle, fields, m, applied) {
                warn!(app = %m.app_id, error = %e, "component search not brought up to date");
                return None;
            }
        }
        Some(Arc::new(RunSearch(entry)))
    }

    /// Forget an app's indexes (it moved off this node, or was deleted).
    pub fn drop_app(&self, app_id: &str) {
        let _gone = self
            .apps
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(app_id);
    }
}

fn refresh(
    ix: &mut AppIndexes,
    bundle: &str,
    fields: &BTreeMap<String, Vec<String>>,
    m: &AppMachine,
    applied: u64,
) -> Result<(), String> {
    if ix.bundle == bundle && ix.built_at == applied {
        return Ok(());
    }
    let kv = &m.state.component.kv;
    if ix.bundle == bundle {
        if let Some(touched) = m.touched_after(ix.built_at) {
            let mut by_table: HashMap<String, Records> = HashMap::new();
            for key in touched.iter().filter_map(|h| hex::decode(h).ok()) {
                let Some(table) = record_table(&key).filter(|t| ix.tables.contains_key(t)) else {
                    continue;
                };
                let value = kv.get(&key).cloned();
                by_table.entry(table).or_default().push((key, value));
            }
            for (table, records) in by_table {
                if let Some(t) = ix.tables.get_mut(&table) {
                    t.apply(&records, applied)?;
                }
            }
            ix.built_at = applied;
            return Ok(());
        }
    }
    // A new bundle, or entries this replica cannot account for: rebuild.
    debug!(app = %m.app_id, applied, "building component search indexes");
    let mut tables = HashMap::new();
    for (table, table_fields) in fields {
        let mut t = TableIndex::new(table, table_fields)?;
        let mut prefix = GENERATION.to_be_bytes().to_vec();
        prefix.push(0x30);
        prefix.extend(
            table
                .bytes()
                .flat_map(|b| if b == 0 { vec![0x00, 0xFF] } else { vec![b] }),
        );
        prefix.extend_from_slice(&[0x00, 0x00, 0x30, b'r', 0x00, 0x00]);
        let records: Records = kv
            .range(prefix.clone()..)
            .take_while(|(k, _)| k.starts_with(&prefix))
            .map(|(k, v)| (k.clone(), Some(v.clone())))
            .collect();
        t.apply(&records, applied)?;
        let _prev = tables.insert(table.clone(), t);
    }
    *ix = AppIndexes {
        bundle: bundle.to_owned(),
        built_at: applied,
        tables,
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(parts: &[&str]) -> Vec<u8> {
        let mut k = GENERATION.to_be_bytes().to_vec();
        for p in parts {
            k.push(0x30);
            k.extend_from_slice(p.as_bytes());
            k.extend_from_slice(&[0, 0]);
        }
        k
    }

    #[test]
    fn only_a_tables_record_keys_are_records() {
        assert_eq!(
            record_table(&stored(&["notes", "r", "1"])),
            Some("notes".into())
        );
        assert_eq!(record_table(&stored(&["notes", "i", "byOwner"])), None);
        assert_eq!(record_table(&stored(&["size", "n"])), None);
        assert_eq!(record_table(b"short"), None);
    }

    #[test]
    fn a_record_indexes_its_declared_text_fields() {
        let mut v = Vec::new();
        ciborium::into_writer(
            &ciborium::Value::Map(vec![
                ("title".into(), "Hello world".into()),
                (
                    "tags".into(),
                    ciborium::Value::Array(vec!["a".into(), "b".into()]),
                ),
                ("secret".into(), "not indexed".into()),
                ("n".into(), 3.into()),
            ]),
            &mut v,
        )
        .unwrap();
        let key = stored(&["notes", "r", "1"]);
        let doc = document(&key, &v, &["title".into(), "tags".into(), "n".into()]).unwrap();
        assert_eq!(doc.fields.len(), 2, "{:?}", doc.fields);
    }
}
