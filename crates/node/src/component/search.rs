//! Full-text search for component apps (spec section 7, R2): the host keeps an index of
//! each table's declared text fields, derived from the app's state, and views query it
//! through the WIT `search` interface.
//!
//! What to index comes from the bundle's ABI (`state.search`: table to fields, from
//! `#[table(search = [..])]`). A table's records are the stored keys
//! `generation ‖ (table, "r", key..)` in the SDK's key encoding (`crates/sdk`, encoding
//! contract); each becomes one document, its id the SHA-256 of the stored key, its fields
//! the declared ones that hold text (a list of strings is joined). The indexes are
//! `calimero-search`'s `ContextIndex` in memory: a view's first query brings them up to the
//! state it reads, from the record keys the entries since touched (`AppMachine::touched_after`,
//! kept for `TOUCHED_RING` entries), else by a full scan of the app's records, on the view's
//! own thread and never under the app's lock. Derived data: never in the log or a snapshot,
//! rebuilt after a restart.
//!
//! A view's searches answer from the state it reads, never another (review finding C4). Its
//! apps' views share one set of indexes, so a view takes a point-in-time view of them (a
//! tantivy `Searcher` and the document keys, per table) as soon as they are brought up to its
//! state, and every search it makes reads that, however far a later view moves the indexes
//! meanwhile. Views queued on earlier states are brought up first, in order (`AppEntry::due`),
//! so the indexes pass through each live view's state instead of jumping over it.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, TryLockError, Weak};

use calimero_app_host::{SearchHit, SearchHost, SearchMode, SearchPage, SearchQuery, GENERATION};
use calimero_primitives::search::{
    SearchDoc, SearchFieldKind, SearchFieldSchema, SearchIndexSchema, SearchMode as IndexMode,
    SearchOrder, SearchRequest, SearchValue,
};
use calimero_search::index::{doc_digest, ContextIndex, SchemaOptions};
use ring::digest::{digest, SHA256};
use tantivy::Searcher;
use tracing::{debug, warn};

use crate::app::AppMachine;
use crate::component::Kv;
use crate::types::AppId;

/// Bytes of the generation prefix on a stored key (D39).
const GENERATION_PREFIX: usize = 8;
/// Bytes of text one document field is indexed from at most; the rest is cut.
pub const MAX_FIELD_TEXT_BYTES: usize = 64 * 1024;
/// Most tantivy writers kept open between refreshes, across every app's tables: those
/// of the apps refreshed last. Each holds three threads (indexing, segment updater,
/// merges) and an arena of up to `calimero_search::index::WRITER_MEMORY` while it
/// indexes; an app refreshed again while it keeps its writers neither reopens them nor
/// waits for the merges its last commit started.
pub const MAX_OPEN_WRITERS: usize = 16;

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

/// Document ids to the stored keys of their records: a persistent map, so a view's
/// point-in-time copy costs nothing and later writes leave it as it was.
type DocKeys = imbl::HashMap<[u8; 32], Vec<u8>>;

/// One table's index and the stored keys of its documents.
struct TableIndex {
    fields: Vec<String>,
    index: Arc<ContextIndex>,
    keys: DocKeys,
}

impl TableIndex {
    /// `table`'s index: in memory, or kept in `dir`, opened as it was left there, or made
    /// anew when there is none or it was made under another schema or format.
    fn open(dir: Option<&Path>, table: &str, fields: &[String]) -> Result<Self, String> {
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
        let open = |dir: Box<dyn tantivy::Directory>| {
            ContextIndex::open(dir, &def, SchemaOptions::default())
                .map(|(index, _state)| index)
                .map_err(|e| e.to_string())
        };
        let index = match dir {
            None => open(Box::new(tantivy::directory::RamDirectory::create()))?
                .ok_or("a new index opened stale")?,
            Some(dir) => {
                let on_disk = || -> Result<Box<dyn tantivy::Directory>, String> {
                    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                    let d =
                        tantivy::directory::MmapDirectory::open(dir).map_err(|e| e.to_string())?;
                    Ok(Box::new(d))
                };
                match open(on_disk()?)? {
                    Some(index) => index,
                    // Made under another schema or format: made anew.
                    None => {
                        std::fs::remove_dir_all(dir).map_err(|e| e.to_string())?;
                        open(on_disk()?)?.ok_or("a new index opened stale")?
                    }
                }
            }
        };
        Ok(Self {
            fields: fields.to_vec(),
            index: Arc::new(index),
            keys: DocKeys::new(),
        })
    }

    /// Bring the index up to `kv` by comparing: every record of `table` is read and its
    /// document's digest compared with the one the index holds, and only the documents
    /// that differ are indexed again (or dropped). On a new index that is every one.
    /// Answers how many documents were indexed or dropped.
    fn reconcile(&mut self, kv: &Kv, table: &str, at: u64) -> Result<u64, String> {
        let mut held = self.index.digests().map_err(|e| e.to_string())?;
        let mut changes = Vec::new();
        self.keys.clear();
        let prefix = record_prefix(table);
        for (key, value) in kv
            .range(prefix.clone()..)
            .take_while(|(k, _)| k.starts_with(&prefix))
        {
            let id = doc_id(key);
            match document(key, value, &self.fields) {
                Some(doc) => {
                    let _prev = self.keys.insert(id, key.clone());
                    if held.remove(&id) != Some(doc_digest(&doc)) {
                        changes.push((id, Some(doc)));
                    }
                }
                None => {
                    if held.remove(&id).is_some() {
                        changes.push((id, None));
                    }
                }
            }
        }
        changes.extend(held.into_keys().map(|id| (id, None)));
        let n = changes.len() as u64;
        self.write(&changes, at)?;
        Ok(n)
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
        self.write(&changes, at)
    }

    /// Index `changes` and commit them as of `at`. The writer stays open for the app's
    /// next refresh until `Writers` releases it; a batch that failed is released at once,
    /// so its uncommitted changes go with its writer and never into a later commit (the
    /// next refresh compares the index with the state).
    fn write(&self, changes: &[([u8; 32], Option<SearchDoc>)], at: u64) -> Result<(), String> {
        let written = self
            .index
            .apply(changes.iter().map(|(id, doc)| (id, doc.as_ref())))
            .and_then(|_n| self.index.commit(at, [0; 32]));
        if written.is_err() {
            if let Err(e) = self.index.release_writer() {
                warn!(error = %e, "a search writer not released");
            }
        }
        written.map_err(|e| e.to_string())
    }
}

/// Tables to the text fields an ABI declares for them.
type Fields = BTreeMap<String, Vec<String>>;

/// An app's indexes, as of `built_at`, of the tables and fields `fields` names.
#[derive(Default)]
struct AppIndexes {
    /// Whether they were ever built (an empty `fields` never is).
    built: bool,
    fields: Fields,
    built_at: u64,
    tables: HashMap<String, TableIndex>,
    /// Tables brought up to a state by comparing every record so far, for tests and logs.
    scans: u64,
    /// Documents those comparisons indexed again or dropped.
    reindexed: u64,
}

impl AppIndexes {
    /// The indexes as they are now, as a view's searches will read them whatever is
    /// committed after.
    fn snapshot(&self) -> Arc<Snapshot> {
        Arc::new(Snapshot {
            tables: self
                .tables
                .iter()
                .map(|(table, t)| {
                    let view = TableSnapshot {
                        index: Arc::clone(&t.index),
                        searcher: t.index.searcher(),
                        keys: t.keys.clone(),
                    };
                    (table.clone(), view)
                })
                .collect(),
        })
    }
}

/// An app's indexes at one state: what a view searches.
struct Snapshot {
    tables: HashMap<String, TableSnapshot>,
}

/// One table's index at that state: the commit its searcher reads, and the stored keys of
/// the documents in it.
struct TableSnapshot {
    index: Arc<ContextIndex>,
    searcher: Searcher,
    keys: DocKeys,
}

/// What an app's indexes were last brought up to: kept apart from the indexes, and
/// only ever locked for a copy, so a view reads it under the app's lock while a
/// rebuild holds the indexes.
#[derive(Clone, Default)]
struct Mark {
    built: bool,
    built_at: u64,
}

/// One app's indexes and their mark.
#[derive(Default)]
struct AppEntry {
    mark: Mutex<Mark>,
    indexes: Mutex<AppIndexes>,
    /// Where its indexes are kept on disk; `None`: in memory.
    dir: Option<PathBuf>,
    /// The views prepared on its state that have not searched yet, in the order they
    /// were prepared (so by the state they read); weak, so a view that ends without
    /// searching leaves nothing behind.
    waiting: Mutex<Vec<Weak<ViewSlot>>>,
}

impl AppEntry {
    /// Queue `slot`, dropping the slots of views that ended meanwhile.
    fn wait(&self, slot: &Arc<ViewSlot>) {
        let mut waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
        waiting.retain(|w| w.strong_count() > 0);
        waiting.push(Arc::downgrade(slot));
    }

    /// The views still running that were prepared on a state up to `applied`, oldest
    /// state first, out of the queue: the indexes are brought up to each in turn.
    fn due(&self, applied: u64) -> Vec<Arc<ViewSlot>> {
        let mut waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
        let mut due = Vec::new();
        waiting.retain(|w| {
            let Some(slot) = w.upgrade() else {
                return false;
            };
            if slot.applied > applied {
                return true;
            }
            due.push(slot);
            false
        });
        due.sort_by_key(|slot| slot.applied);
        due
    }

    /// Bring the indexes `ix` up to `slot`'s state and give it their snapshot, unless it
    /// has one already. Called with the indexes locked.
    fn bring_up(&self, ix: &mut AppIndexes, slot: &ViewSlot) {
        let mut state = slot.state.lock().unwrap_or_else(PoisonError::into_inner);
        let Prepared::Pending(p) = &*state else {
            return;
        };
        *state = match refresh(ix, p, self.dir.as_deref()) {
            Ok(()) => {
                *self.mark.lock().unwrap_or_else(PoisonError::into_inner) = Mark {
                    built: ix.built,
                    built_at: ix.built_at,
                };
                Prepared::Ready(ix.snapshot())
            }
            Err(e) => {
                warn!(app = %p.app_id, error = %e, "component search not brought up to date");
                Prepared::Failed(format!("search is unavailable: {e}"))
            }
        };
    }

    fn mark(&self) -> Mark {
        self.mark
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Release every table's writer; each finishes its merges on a thread of its own.
    /// `false` while a refresh or a query holds the indexes (never waited for: a full
    /// scan of another app may take long).
    fn release_writers(&self, app_id: &str) -> bool {
        let ix = match self.indexes.try_lock() {
            Ok(ix) => ix,
            Err(TryLockError::Poisoned(p)) => p.into_inner(),
            Err(TryLockError::WouldBlock) => return false,
        };
        for t in ix.tables.values() {
            if let Err(e) = t.index.release_writer() {
                warn!(app = %app_id, error = %e, "a search writer not released");
            }
        }
        true
    }
}

/// The apps whose indexes may hold writers, least recently refreshed first, with how
/// many tables each has. Review finding C1: every index kept its writer from its first
/// write until its app left the node, so 200 apps of 3 searchable tables held some 1,800
/// OS threads. Past `MAX_OPEN_WRITERS` tables, the apps refreshed longest ago give theirs
/// up; the app refreshed last keeps its own whatever its count.
#[derive(Default)]
struct Writers {
    held: Mutex<VecDeque<(AppId, Arc<AppEntry>, usize)>>,
}

impl Writers {
    /// `app_id` was just refreshed and its indexes' lock let go: it goes last, and the
    /// apps first in line past `MAX_OPEN_WRITERS` tables release their writers.
    fn refreshed(&self, app_id: &str, entry: &Arc<AppEntry>, tables: usize) {
        let released: Vec<_> = {
            let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
            held.retain(|(app, ..)| app != app_id);
            held.push_back((app_id.to_owned(), Arc::clone(entry), tables));
            let mut open: usize = held.iter().map(|(.., n)| n).sum();
            let mut released = Vec::new();
            while open > MAX_OPEN_WRITERS && held.len() > 1 {
                let Some(oldest) = held.pop_front() else {
                    break;
                };
                open -= oldest.2;
                released.push(oldest);
            }
            released
        };
        let busy: Vec<_> = released
            .into_iter()
            .filter(|(app, entry, _)| !entry.release_writers(app))
            .collect();
        if busy.is_empty() {
            return;
        }
        // Busy: back first in line, for the next refresh to release (one that was
        // refreshing has put itself back last already).
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        for item in busy.into_iter().rev() {
            if !held.iter().any(|(app, ..)| *app == item.0) {
                held.push_front(item);
            }
        }
    }

    fn forget(&self, app_id: &str) {
        self.held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(app, ..)| app != app_id);
    }
}

/// Full-text search over the component apps hosted here: in memory, or kept on disk.
#[derive(Default)]
pub struct ComponentSearch {
    apps: Mutex<HashMap<AppId, Arc<AppEntry>>>,
    /// Where indexes are kept (`<data dir>/search`), only on a data disk the node knows is
    /// encrypted; `None`: in memory.
    root: Option<PathBuf>,
    writers: Arc<Writers>,
}

/// What a view's state holds for its search: taken under the app's read lock (an
/// `Arc` of the state and the record keys touched since the mark, never an index),
/// and indexed only when the view first searches, on the view's own thread.
struct Pending {
    app_id: AppId,
    fields: Fields,
    kv: Kv,
    applied: u64,
    /// The mark the touched keys were read against.
    from: Mark,
    /// Record keys the entries after `from.built_at` touched; `None`: rebuild.
    touched: Option<Vec<String>>,
}

/// Where a view's search stands: not yet brought up to its state, or ready with the
/// indexes as of that state (or refused, with why).
enum Prepared {
    Pending(Box<Pending>),
    Ready(Arc<Snapshot>),
    Failed(String),
}

/// A view's place in its app's queue: the state it reads, and its search's.
struct ViewSlot {
    applied: u64,
    state: Mutex<Prepared>,
}

impl ViewSlot {
    /// The view's snapshot, or why it has none; `None` while it is pending.
    fn done(&self) -> Option<Result<Arc<Snapshot>, String>> {
        match &*self.state.lock().unwrap_or_else(PoisonError::into_inner) {
            Prepared::Pending(_) => None,
            Prepared::Ready(snapshot) => Some(Ok(Arc::clone(snapshot))),
            Prepared::Failed(e) => Some(Err(e.clone())),
        }
    }
}

/// A view's search, bound to its app's indexes.
pub struct RunSearch {
    app_id: AppId,
    entry: Arc<AppEntry>,
    writers: Arc<Writers>,
    slot: Arc<ViewSlot>,
}

impl RunSearch {
    /// The indexes as of the view's state: brought up to it on the first query, and the
    /// same snapshot for every query after.
    ///
    /// The views queued on states up to this one's are brought up first, oldest first, each
    /// taking its own snapshot: so the indexes never move past a view that has yet to
    /// search, and none of them has to bring them back down to its state.
    fn ready(&self) -> Result<Arc<Snapshot>, String> {
        if let Some(done) = self.slot.done() {
            return done;
        }
        let mut ix = self
            .entry
            .indexes
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for slot in self.entry.due(self.slot.applied) {
            self.entry.bring_up(&mut ix, &slot);
        }
        // Taken out of the queue by another view that brought it up meanwhile, or by this
        // one just now; brought up here in case neither did.
        self.entry.bring_up(&mut ix, &self.slot);
        let tables = ix.tables.len();
        drop(ix);
        self.writers.refreshed(&self.app_id, &self.entry, tables);
        self.slot
            .done()
            .unwrap_or_else(|| Err("search is unavailable: not brought up".to_owned()))
    }
}

impl SearchHost for RunSearch {
    fn query(&self, q: &SearchQuery) -> Result<SearchPage, String> {
        let snapshot = self.ready()?;
        let Some(table) = snapshot.tables.get(&q.table) else {
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
        let response = table
            .index
            .search_in(&table.searcher, &request)
            .map_err(|e| e.to_string())?;
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
    /// Indexes kept under `root` when given, else in memory.
    ///
    /// tantivy writes its files as they are, so `root` must be on a data disk the TD
    /// encrypts (mero-tee's LUKS2 volume, keyed by mero-kms): the node passes one only
    /// when it was started with the record of that disk's key (`--kms-disk-evidence`).
    /// An index kept there is never trusted to be current: it is compared with the
    /// state on its first use, and only what differs is indexed again.
    #[must_use]
    pub fn new(root: Option<PathBuf>) -> Self {
        Self {
            apps: Mutex::default(),
            root,
            writers: Arc::default(),
        }
    }

    /// The app's search for a view on the state `m` holds at `applied`, whose bundle's
    /// ABI declares `fields` (table to text fields); `None` when it declares none.
    ///
    /// Called under the app's read lock, so it indexes nothing: it keeps the state (an
    /// `Arc`) and the record keys touched since the indexes were last brought up, and the
    /// view's first query brings them up to that state, on the view's own thread (off the
    /// async workers, outside the lock). A view that never searches costs no indexing. An
    /// index that cannot be built answers that query with an error.
    pub fn for_view(
        &self,
        fields: &Fields,
        m: &AppMachine,
        applied: u64,
    ) -> Option<Arc<dyn SearchHost>> {
        self.prepare(
            &m.app_id,
            fields,
            m.state.component.kv.clone(),
            applied,
            |since| m.touched_after(since),
        )
    }

    fn prepare(
        &self,
        app_id: &str,
        fields: &Fields,
        kv: Kv,
        applied: u64,
        touched_after: impl FnOnce(u64) -> Option<Vec<String>>,
    ) -> Option<Arc<dyn SearchHost>> {
        if fields.is_empty() {
            return None;
        }
        let entry = {
            let mut apps = self.apps.lock().unwrap_or_else(PoisonError::into_inner);
            Arc::clone(apps.entry(app_id.to_owned()).or_insert_with(|| {
                Arc::new(AppEntry {
                    dir: self.root.as_deref().map(|r| app_dir(r, app_id)),
                    ..AppEntry::default()
                })
            }))
        };
        let from = entry.mark();
        // Whatever the bundle: an upgrade that keeps a table's fields keeps its index,
        // which catches up from these keys like any other.
        let touched = (from.built && from.built_at < applied)
            .then(|| touched_after(from.built_at))
            .flatten();
        let pending = Pending {
            app_id: app_id.to_owned(),
            fields: fields.clone(),
            kv,
            applied,
            from,
            touched,
        };
        let slot = Arc::new(ViewSlot {
            applied,
            state: Mutex::new(Prepared::Pending(Box::new(pending))),
        });
        entry.wait(&slot);
        Some(Arc::new(RunSearch {
            app_id: app_id.to_owned(),
            entry,
            writers: Arc::clone(&self.writers),
            slot,
        }))
    }

    /// Forget an app's indexes (it moved off this node, or was deleted), on disk too.
    pub fn drop_app(&self, app_id: &str) {
        let _gone = self
            .apps
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(app_id);
        self.writers.forget(app_id);
        if let Some(root) = &self.root {
            let dir = app_dir(root, app_id);
            if let Err(e) = std::fs::remove_dir_all(&dir) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    warn!(app = %app_id, error = %e, "search indexes not removed from disk");
                }
            }
        }
    }
}

/// Bring `ix` up to the view's state `p`; indexes are kept under `dir` when given.
///
/// Incremental where it can be: from the record keys the entries since touched, for
/// every table whose declared fields are what its index was made with, also across an
/// upgrade. The indexes hold exactly the state at `built_at`, so the keys the entries
/// after the view's mark touched cover what changed from any `built_at` between that
/// mark and the view's state. When the keys since are unknown (the first search after a
/// start, more than `TOUCHED_RING` entries since), or the indexes are outside that span
/// (a view on an earlier state than they hold), each index is
/// compared with the state instead (`TableIndex::reconcile`), which indexes only the
/// documents that differ: an index kept on disk from before a restart catches up so. A
/// table that is new or whose fields changed starts from a new index; one no longer
/// declared is dropped, with its files.
fn refresh(ix: &mut AppIndexes, p: &Pending, dir: Option<&Path>) -> Result<(), String> {
    if ix.built && ix.fields == p.fields && ix.built_at == p.applied {
        return Ok(());
    }
    let delta = if !ix.built || ix.built_at < p.from.built_at || ix.built_at > p.applied {
        None
    } else if ix.built_at == p.applied {
        Some(Vec::new())
    } else {
        p.touched.clone()
    };
    // Built again from here on: a failure part way leaves the indexes to be compared
    // with the state next time.
    ix.built = false;
    let mut tables = std::mem::take(&mut ix.tables);
    let gone: Vec<String> = tables
        .iter()
        .filter(|(table, t)| p.fields.get(*table) != Some(&t.fields))
        .map(|(table, _)| table.clone())
        .collect();
    for table in gone {
        drop(tables.remove(&table));
        if let Some(dir) = dir {
            let _gone = std::fs::remove_dir_all(table_dir(dir, &table));
        }
    }
    match delta {
        Some(touched) => {
            let mut by_table: HashMap<String, Records> = HashMap::new();
            for key in touched.iter().filter_map(|h| hex::decode(h).ok()) {
                let Some(table) = record_table(&key).filter(|t| tables.contains_key(t)) else {
                    continue;
                };
                let value = p.kv.get(&key).cloned();
                by_table.entry(table).or_default().push((key, value));
            }
            for (table, records) in by_table {
                if let Some(t) = tables.get_mut(&table) {
                    t.apply(&records, p.applied)?;
                }
            }
        }
        None => {
            debug!(app = %p.app_id, applied = p.applied, "comparing component search indexes with the state");
            for (table, t) in &mut tables {
                ix.reindexed += t.reconcile(&p.kv, table, p.applied)?;
                ix.scans += 1;
            }
        }
    }
    for (table, table_fields) in &p.fields {
        if tables.contains_key(table) {
            continue;
        }
        debug!(app = %p.app_id, table, "opening a component search index");
        let path = dir.map(|d| table_dir(d, table));
        let mut t = TableIndex::open(path.as_deref(), table, table_fields)?;
        ix.reindexed += t.reconcile(&p.kv, table, p.applied)?;
        ix.scans += 1;
        let _prev = tables.insert(table.clone(), t);
    }
    ix.tables = tables;
    ix.fields = p.fields.clone();
    ix.built_at = p.applied;
    ix.built = true;
    Ok(())
}

/// The stored-key prefix of `table`'s records: `generation ‖ (table, "r", ..)`.
fn record_prefix(table: &str) -> Vec<u8> {
    let mut prefix = GENERATION.to_be_bytes().to_vec();
    prefix.push(0x30);
    prefix.extend(
        table
            .bytes()
            .flat_map(|b| if b == 0 { vec![0x00, 0xFF] } else { vec![b] }),
    );
    prefix.extend_from_slice(&[0x00, 0x00, 0x30, b'r', 0x00, 0x00]);
    prefix
}

/// An app's directory under `root`: named by the SHA-256 of its id, so any id is a path.
fn app_dir(root: &Path, app_id: &str) -> PathBuf {
    root.join(hex::encode(digest(&SHA256, app_id.as_bytes()).as_ref()))
}

/// A table's index directory under its app's.
fn table_dir(app_dir: &Path, table: &str) -> PathBuf {
    app_dir.join(hex::encode(digest(&SHA256, table.as_bytes()).as_ref()))
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

    fn note(title: &str) -> Vec<u8> {
        let mut v = Vec::new();
        ciborium::into_writer(
            &ciborium::Value::Map(vec![("title".into(), title.into())]),
            &mut v,
        )
        .unwrap();
        v
    }

    fn fields() -> BTreeMap<String, Vec<String>> {
        [("notes".to_owned(), vec!["title".to_owned()])].into()
    }

    fn state(notes: &[(&str, &str)]) -> Kv {
        notes
            .iter()
            .map(|(id, title)| (stored(&["notes", "r", id]), note(title)))
            .collect()
    }

    fn find(search: &dyn SearchHost, text: &str) -> Vec<Vec<u8>> {
        let mut keys: Vec<_> = search
            .query(&SearchQuery {
                table: "notes".into(),
                text: text.into(),
                mode: SearchMode::Words,
                cursor: 0,
                limit: 10,
            })
            .unwrap()
            .hits
            .into_iter()
            .map(|h| h.key)
            .collect();
        keys.sort();
        keys
    }

    fn key(id: &str) -> Vec<u8> {
        stored(&["notes", "r", id])[GENERATION_PREFIX..].to_vec()
    }

    fn built_at(search: &ComponentSearch, app: &str) -> Option<u64> {
        let apps = search.apps.lock().unwrap();
        let entry = apps.get(app)?;
        let ix = entry.indexes.lock().unwrap();
        (!ix.tables.is_empty()).then_some(ix.built_at)
    }

    /// Review finding A5: the index was refreshed, or rebuilt in full, inside
    /// `for_view`, which runs under the app's read lock on an async worker, for
    /// every view, searching or not. Preparing a view's search now indexes
    /// nothing; the view's first query does, on the view's own thread.
    #[test]
    fn preparing_a_views_search_indexes_nothing_until_it_queries() {
        let search = ComponentSearch::default();
        let kv = state(&[("1", "hello world"), ("2", "other")]);
        let prepared = search
            .prepare("app", &fields(), kv, 5, |_| {
                panic!("no mark yet: nothing to catch up from")
            })
            .unwrap();
        assert_eq!(built_at(&search, "app"), None, "nothing indexed yet");
        assert_eq!(find(&*prepared, "hello"), vec![key("1")]);
        assert_eq!(built_at(&search, "app"), Some(5));
    }

    /// The next view catches up from the keys the entries since touched, read from its
    /// own state.
    #[test]
    fn a_later_view_catches_up_from_the_keys_touched_since() {
        let search = ComponentSearch::default();
        let first = search
            .prepare("app", &fields(), state(&[("1", "hello")]), 5, |_| None)
            .unwrap();
        assert_eq!(find(&*first, "hello"), vec![key("1")]);
        let touched = vec![hex::encode(stored(&["notes", "r", "2"]))];
        let later = search
            .prepare(
                "app",
                &fields(),
                state(&[("1", "hello"), ("2", "hello again")]),
                7,
                |since| {
                    assert_eq!(since, 5);
                    Some(touched)
                },
            )
            .unwrap();
        assert_eq!(find(&*later, "hello"), vec![key("1"), key("2")]);
        assert_eq!(built_at(&search, "app"), Some(7));
    }

    /// A view whose indexes another view, on a later state under other fields (an
    /// upgrade), moves before it queries searches its own state: it is brought up to it
    /// before the later view moves them on.
    #[test]
    fn a_view_whose_indexes_a_later_upgrade_moved_searches_its_own_state() {
        let search = ComponentSearch::default();
        let base = search
            .prepare("app", &fields(), state(&[("1", "hello")]), 5, |_| None)
            .unwrap();
        assert_eq!(find(&*base, "hello"), vec![key("1")]);
        // Prepared at 6 against the mark at 5, queried after another view moved the
        // indexes to 8 for other fields (an upgrade), from a state the keys since 5
        // cannot describe.
        let stale = search
            .prepare(
                "app",
                &fields(),
                state(&[("1", "hello"), ("3", "hello three")]),
                6,
                |_| Some(vec![hex::encode(stored(&["notes", "r", "3"]))]),
            )
            .unwrap();
        let upgraded: Fields = [(
            "notes".to_owned(),
            vec!["title".to_owned(), "body".to_owned()],
        )]
        .into();
        let other = search
            .prepare("app", &upgraded, state(&[("9", "hello nine")]), 8, |_| None)
            .unwrap();
        assert_eq!(find(&*other, "hello"), vec![key("9")]);
        assert_eq!(find(&*stale, "hello"), vec![key("1"), key("3")]);
        assert_eq!(built_at(&search, "app"), Some(8));
    }

    fn touched(ids: &[&str]) -> Vec<String> {
        ids.iter()
            .map(|id| hex::encode(stored(&["notes", "r", id])))
            .collect()
    }

    /// Review finding C4: an app's views share its indexes, and every query read their
    /// latest commit, so a view on the state at 5 searched whatever a later view had
    /// brought them to: a hit for a record its state does not hold yet, and none for
    /// one the later state deleted. A view's searches answer from the indexes as of the
    /// state it reads, however far another view moves them between its searches.
    #[test]
    fn a_view_searches_the_state_it_reads_while_a_later_view_moves_the_indexes() {
        let search = ComponentSearch::default();
        let at5 = search
            .prepare(
                "app",
                &fields(),
                state(&[("1", "hello"), ("2", "hello two")]),
                5,
                |_| None,
            )
            .unwrap();
        assert_eq!(find(&*at5, "hello"), vec![key("1"), key("2")]);
        // Entry 6 deletes 2 and adds 3; a view on it searches.
        let at6 = search
            .prepare(
                "app",
                &fields(),
                state(&[("1", "hello"), ("3", "hello three")]),
                6,
                |since| {
                    assert_eq!(since, 5);
                    Some(touched(&["2", "3"]))
                },
            )
            .unwrap();
        assert_eq!(find(&*at6, "hello"), vec![key("1"), key("3")]);
        // The view at 5 searches again: still the state at 5.
        assert_eq!(find(&*at5, "hello"), vec![key("1"), key("2")]);
    }

    /// C4, the other order: a view prepared on the state at 5 that first searches after a
    /// view on 6 brought the indexes to 6 searches the state at 5, not 6.
    #[test]
    fn a_view_that_searches_after_a_later_view_did_searches_its_own_state() {
        let search = ComponentSearch::default();
        let base = search
            .prepare("app", &fields(), state(&[("1", "hello")]), 4, |_| None)
            .unwrap();
        assert_eq!(find(&*base, "hello"), vec![key("1")]);
        let at5 = search
            .prepare(
                "app",
                &fields(),
                state(&[("1", "hello"), ("2", "hello two")]),
                5,
                |_| Some(touched(&["2"])),
            )
            .unwrap();
        // Entry 6 deletes 1 and adds 3.
        let at6 = search
            .prepare(
                "app",
                &fields(),
                state(&[("2", "hello two"), ("3", "hello three")]),
                6,
                |_| Some(touched(&["2", "1", "3"])),
            )
            .unwrap();
        assert_eq!(find(&*at6, "hello"), vec![key("2"), key("3")]);
        assert_eq!(find(&*at5, "hello"), vec![key("1"), key("2")]);
        assert_eq!(
            scans(&search, "app"),
            1,
            "caught up from the keys, never scanned"
        );
    }

    /// A view on a state older than the indexes hold (never in order, but the indexes do
    /// not rely on it) compares them with its own state instead of patching them.
    #[test]
    fn a_view_on_an_older_state_than_the_indexes_compares_them_with_it() {
        let search = ComponentSearch::default();
        let at6 = search
            .prepare("app", &fields(), state(&[("3", "hello three")]), 6, |_| {
                None
            })
            .unwrap();
        assert_eq!(find(&*at6, "hello"), vec![key("3")]);
        let at5 = search
            .prepare("app", &fields(), state(&[("1", "hello")]), 5, |_| None)
            .unwrap();
        assert_eq!(find(&*at5, "hello"), vec![key("1")]);
        assert_eq!(
            find(&*at6, "hello"),
            vec![key("3")],
            "its own snapshot still"
        );
    }

    /// Views that end without searching leave nothing queued.
    #[test]
    fn views_that_never_search_leave_nothing_queued() {
        let search = ComponentSearch::default();
        for applied in 1..=100 {
            let view = search.prepare("app", &fields(), state(&[]), applied, |_| None);
            drop(view);
        }
        let entry = Arc::clone(&search.apps.lock().unwrap()["app"]);
        assert!(entry.waiting.lock().unwrap().len() <= 1);
        assert_eq!(built_at(&search, "app"), None, "nothing indexed for them");
    }

    #[test]
    fn an_app_that_declares_no_search_gets_none() {
        let search = ComponentSearch::default();
        assert!(search
            .prepare("app", &BTreeMap::new(), state(&[]), 1, |_| None)
            .is_none());
    }

    fn scans(search: &ComponentSearch, app: &str) -> u64 {
        let apps = search.apps.lock().unwrap();
        let ix = apps[app].indexes.lock().unwrap();
        ix.scans
    }

    fn find_in(search: &dyn SearchHost, table: &str, text: &str) -> Result<usize, String> {
        search
            .query(&SearchQuery {
                table: table.into(),
                text: text.into(),
                mode: SearchMode::Words,
                cursor: 0,
                limit: 10,
            })
            .map(|page| page.hits.len())
    }

    fn with_tags(notes: &[(&str, &str)], tags: &[(&str, &str)]) -> Kv {
        let mut kv: BTreeMap<Vec<u8>, Vec<u8>> = notes
            .iter()
            .map(|(id, title)| (stored(&["notes", "r", id]), note(title)))
            .collect();
        for (id, title) in tags {
            let _prev = kv.insert(stored(&["tags", "r", id]), note(title));
        }
        kv.into_iter().collect()
    }

    /// Review finding A5 follow-up: an upgrade rebuilt every table from a scan of the
    /// whole state, because the indexes were keyed by bundle. A table whose declared
    /// fields an upgrade keeps now catches up from the keys touched since, like any
    /// other view; only a table that is new or whose fields changed is scanned, and one
    /// the ABI no longer declares is dropped.
    #[test]
    fn an_upgrade_keeps_the_index_of_a_table_whose_fields_it_keeps() {
        let search = ComponentSearch::default();
        let v1: Fields = [
            ("notes".to_owned(), vec!["title".to_owned()]),
            ("tags".to_owned(), vec!["title".to_owned()]),
        ]
        .into();
        let first = search
            .prepare(
                "app",
                &v1,
                with_tags(&[("1", "hello")], &[("t", "hello")]),
                5,
                |_| None,
            )
            .unwrap();
        assert_eq!(find_in(&*first, "notes", "hello"), Ok(1));
        assert_eq!(scans(&search, "app"), 2, "both tables scanned once");

        // The upgrade keeps `notes` as it was, drops `tags` and adds `labels`.
        let v2: Fields = [
            ("notes".to_owned(), vec!["title".to_owned()]),
            ("labels".to_owned(), vec!["title".to_owned()]),
        ]
        .into();
        let touched = vec![hex::encode(stored(&["notes", "r", "2"]))];
        let upgraded = search
            .prepare(
                "app",
                &v2,
                with_tags(&[("1", "hello"), ("2", "hello again")], &[("t", "hello")]),
                9,
                |since| {
                    assert_eq!(since, 5);
                    Some(touched)
                },
            )
            .unwrap();
        assert_eq!(find_in(&*upgraded, "notes", "hello"), Ok(2), "caught up");
        assert_eq!(scans(&search, "app"), 3, "only the new table was scanned");
        assert!(find_in(&*upgraded, "tags", "hello").is_err(), "dropped");
        assert_eq!(find_in(&*upgraded, "labels", "hello"), Ok(0));

        // A changed field list scans that table again.
        let v3: Fields = [
            (
                "notes".to_owned(),
                vec!["title".to_owned(), "body".to_owned()],
            ),
            ("labels".to_owned(), vec!["title".to_owned()]),
        ]
        .into();
        let changed = search
            .prepare("app", &v3, with_tags(&[("1", "hello")], &[]), 10, |_| {
                Some(Vec::new())
            })
            .unwrap();
        assert_eq!(find_in(&*changed, "notes", "hello"), Ok(1));
        assert_eq!(scans(&search, "app"), 4, "notes again, labels kept");
    }

    /// When the keys since are unknown (the ring was passed), every table is scanned.
    #[test]
    fn indexes_the_ring_cannot_catch_up_are_scanned_again() {
        let search = ComponentSearch::default();
        let first = search
            .prepare("app", &fields(), state(&[("1", "hello")]), 5, |_| None)
            .unwrap();
        assert_eq!(find(&*first, "hello"), vec![key("1")]);
        let later = search
            .prepare("app", &fields(), state(&[("2", "hello")]), 9_000, |_| None)
            .unwrap();
        assert_eq!(find(&*later, "hello"), vec![key("2")]);
        assert_eq!(scans(&search, "app"), 2);
    }

    fn reindexed(search: &ComponentSearch, app: &str) -> u64 {
        let apps = search.apps.lock().unwrap();
        let ix = apps[app].indexes.lock().unwrap();
        ix.reindexed
    }

    /// Indexes kept on disk survive a restart: the new process compares them with the
    /// state and indexes again only the documents that changed, appeared or went.
    #[test]
    fn an_index_kept_on_disk_catches_up_after_a_restart_reindexing_only_what_changed() {
        let dir = tempfile::tempdir().unwrap();
        {
            let before = ComponentSearch::new(Some(dir.path().to_owned()));
            let first = before
                .prepare(
                    "app",
                    &fields(),
                    state(&[("1", "hello"), ("2", "hello two"), ("3", "hello three")]),
                    5,
                    |_| None,
                )
                .unwrap();
            assert_eq!(find(&*first, "hello"), vec![key("1"), key("2"), key("3")]);
            assert_eq!(reindexed(&before, "app"), 3);
        }
        // A restart: nothing in memory, and the entries since are unknown.
        let after = ComponentSearch::new(Some(dir.path().to_owned()));
        let view = after
            .prepare(
                "app",
                &fields(),
                state(&[("1", "hello"), ("2", "goodbye two"), ("4", "hello four")]),
                9,
                |_| None,
            )
            .unwrap();
        assert_eq!(find(&*view, "hello"), vec![key("1"), key("4")]);
        assert_eq!(find(&*view, "goodbye"), vec![key("2")]);
        assert_eq!(
            reindexed(&after, "app"),
            3,
            "2 changed, 3 went, 4 appeared; 1 was left as it was"
        );

        after.drop_app("app");
        assert!(
            !app_dir(dir.path(), "app").exists(),
            "a dropped app's files go too"
        );
    }

    /// An index left on disk under another schema is made anew, never read as current.
    #[test]
    fn an_index_on_disk_under_other_fields_is_made_anew() {
        let dir = tempfile::tempdir().unwrap();
        {
            let before = ComponentSearch::new(Some(dir.path().to_owned()));
            let first = before
                .prepare("app", &fields(), state(&[("1", "hello")]), 5, |_| None)
                .unwrap();
            assert_eq!(find(&*first, "hello"), vec![key("1")]);
        }
        let other: Fields = [(
            "notes".to_owned(),
            vec!["title".to_owned(), "body".to_owned()],
        )]
        .into();
        let after = ComponentSearch::new(Some(dir.path().to_owned()));
        let view = after
            .prepare("app", &other, state(&[("2", "hello again")]), 6, |_| None)
            .unwrap();
        assert_eq!(find(&*view, "hello"), vec![key("2")]);
        assert_eq!(reindexed(&after, "app"), 1);
    }

    /// In memory too, a comparison after the ring was passed indexes only what differs.
    #[test]
    fn a_comparison_in_memory_indexes_only_what_differs() {
        let search = ComponentSearch::default();
        let first = search
            .prepare(
                "app",
                &fields(),
                state(&[("1", "hello"), ("2", "hello")]),
                5,
                |_| None,
            )
            .unwrap();
        assert_eq!(find(&*first, "hello").len(), 2);
        let later = search
            .prepare(
                "app",
                &fields(),
                state(&[("1", "hello"), ("2", "bye")]),
                9_000,
                |_| None,
            )
            .unwrap();
        assert_eq!(find(&*later, "hello"), vec![key("1")]);
        assert_eq!(
            reindexed(&search, "app"),
            3,
            "2 at first, then only 2 again"
        );
    }

    /// Index writers open across every app's tables, or one app's.
    fn open_writers(search: &ComponentSearch, only: Option<&str>) -> usize {
        let apps = search.apps.lock().unwrap();
        apps.iter()
            .filter(|(app, _)| only.is_none_or(|o| o == app.as_str()))
            .map(|(_, entry)| {
                let ix = entry.indexes.lock().unwrap();
                ix.tables.values().filter(|t| t.index.has_writer()).count()
            })
            .sum()
    }

    /// Review finding C1: every index kept its tantivy writer (an indexing thread, a
    /// segment updater, a merge thread and an arena of up to 15 MB) from its first write
    /// until the app left the node, so 200 apps of 3 searchable tables held some 1,800 OS
    /// threads. Only the apps refreshed last keep theirs, `MAX_OPEN_WRITERS` across every
    /// app's tables; the others give them up, and their next refresh opens them again
    /// (on disk too, where a writer holds the index's lock file until its merges end).
    #[test]
    fn at_most_max_open_writers_stay_open_across_every_apps_indexes() {
        let dir = tempfile::tempdir().unwrap();
        let tables: Fields = [
            ("notes".to_owned(), vec!["title".to_owned()]),
            ("tags".to_owned(), vec!["title".to_owned()]),
        ]
        .into();
        for search in [
            ComponentSearch::default(),
            ComponentSearch::new(Some(dir.path().to_owned())),
        ] {
            for app in 0..20 {
                let view = search
                    .prepare(
                        &format!("app{app}"),
                        &tables,
                        with_tags(&[("1", "hello")], &[("t", "hello")]),
                        5,
                        |_| None,
                    )
                    .unwrap();
                assert_eq!(find_in(&*view, "notes", "hello"), Ok(1));
                assert_eq!(find_in(&*view, "tags", "hello"), Ok(1));
            }
            assert!(
                open_writers(&search, None) <= MAX_OPEN_WRITERS,
                "{} writers open over 40 indexes",
                open_writers(&search, None)
            );
            assert_eq!(
                open_writers(&search, Some("app19")),
                2,
                "the latest keeps its own"
            );
            assert_eq!(
                open_writers(&search, Some("app0")),
                0,
                "the oldest gave its up"
            );

            // The oldest app's next write opens its writers again, and its index answers
            // what it wrote; the bound holds.
            let touched = vec![hex::encode(stored(&["notes", "r", "2"]))];
            let later = search
                .prepare(
                    "app0",
                    &tables,
                    with_tags(&[("1", "hello"), ("2", "hello again")], &[("t", "hello")]),
                    7,
                    |_| Some(touched),
                )
                .unwrap();
            assert_eq!(find(&*later, "hello"), vec![key("1"), key("2")]);
            assert_eq!(built_at(&search, "app0"), Some(7));
            assert_eq!(
                open_writers(&search, Some("app0")),
                1,
                "only notes was written"
            );
            assert!(open_writers(&search, None) <= MAX_OPEN_WRITERS);

            // And one compared with the state (a restart, the ring passed) too.
            let compared = search
                .prepare(
                    "app1",
                    &tables,
                    with_tags(&[("1", "bye")], &[("t", "hello")]),
                    9_000,
                    |_| None,
                )
                .unwrap();
            assert_eq!(find_in(&*compared, "notes", "hello"), Ok(0));
            assert_eq!(find_in(&*compared, "notes", "bye"), Ok(1));
            assert!(open_writers(&search, None) <= MAX_OPEN_WRITERS);
        }
    }

    /// An app whose writers are due to go while a query holds its indexes is not waited
    /// for, and not forgotten either: the next refresh releases them.
    #[test]
    fn writers_due_to_go_while_their_indexes_are_busy_go_with_the_next_refresh() {
        let search = ComponentSearch::default();
        let view = |app: &str| {
            let v = search
                .prepare(app, &fields(), state(&[("1", "hello")]), 5, |_| None)
                .unwrap();
            assert_eq!(find(&*v, "hello"), vec![key("1")]);
        };
        view("busy");
        let entry = Arc::clone(&search.apps.lock().unwrap()["busy"]);
        {
            let _query = entry.indexes.lock().unwrap();
            for app in 0..MAX_OPEN_WRITERS {
                view(&format!("app{app}"));
            }
        }
        assert_eq!(open_writers(&search, Some("busy")), 1, "not waited for");
        view("one more");
        assert_eq!(open_writers(&search, Some("busy")), 0);
        assert!(open_writers(&search, None) <= MAX_OPEN_WRITERS);
    }
}
