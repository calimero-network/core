//! The node's search service: open indexes, the indexer loop, the query entry.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use calimero_primitives::search::{
    ExtractResponse, ScanResponse, SearchIndexSchema, SearchRequest, SearchResponse,
};
use calimero_primitives::utils::prefix_upper_bound;
use calimero_store::db::Column;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::directory::{self, ChunkCache, RocksDirectory};
use crate::dirty;
use crate::index::{ContextIndex, OpenState, SchemaOptions};

/// A context id, as the store keys it.
pub type ContextKey = [u8; 32];

/// Full builds one [`SearchService::index_context`] pass may start for one
/// index before it gives up until the next tick. A second build is needed when
/// state moved without a dirty row *during* the first; a third in a row means
/// something keeps writing that way, and looping on it would starve every
/// other context.
const MAX_BUILDS_PER_PASS: usize = 2;

/// What the indexer needs from the node about a context: the app's search
/// exports, run read-only against current state, and the state itself.
#[async_trait]
pub trait ContextSource: Send + Sync + 'static {
    /// The app's indexes, or `None` if the app declares none.
    async fn schema(&self, context: ContextKey) -> EyreResult<Option<Vec<SearchIndexSchema>>>;

    /// One document (or `None`) per id, in order.
    async fn extract(
        &self,
        context: ContextKey,
        index: &str,
        ids: Vec<[u8; 32]>,
    ) -> EyreResult<ExtractResponse>;

    /// One page of every document of `index`.
    async fn scan(
        &self,
        context: ContextKey,
        index: &str,
        offset: u32,
        limit: u32,
    ) -> EyreResult<ScanResponse>;

    /// The context's current state root.
    ///
    /// # Errors
    /// A store failure.
    fn state_root(&self, context: ContextKey) -> EyreResult<[u8; 32]>;

    /// Hold off every execution and every locked sync apply of `context`
    /// until the returned guard drops, so the state root and the dirty-log
    /// head can be read as one.
    async fn lock(&self, context: ContextKey) -> EyreResult<Box<dyn Send>>;
}

/// Service knobs. [`Default`] is what a node runs unless its operator says
/// otherwise.
#[derive(Clone, Copy, Debug)]
pub struct SearchConfig {
    /// How often the indexer commits what it has.
    pub commit_interval: Duration,
    /// Dirty rows folded into one commit at most.
    pub max_rows_per_commit: usize,
    /// Ids per `extract` call.
    pub extract_batch: usize,
    /// Documents per `scan` page.
    pub scan_page: u32,
    /// Bytes of index chunks cached across all contexts.
    pub cache_bytes: usize,
    /// Indexes kept open at most; the least recently used idle ones close
    /// past it. Each open reader costs about 1.6 MiB.
    pub max_open_indexes: usize,
    /// An index no query or pass used for this long is closed.
    pub reader_idle: Duration,
    /// A writer (a 15 MB arena and a merge thread) idle for this long is
    /// closed.
    pub writer_idle: Duration,
    /// How often every context indexed since start is checked for state that
    /// moved without a dirty row (a sync that installed or repaired state).
    pub audit_interval: Duration,
    /// Bytes of deleted index files after which a context's slice of the
    /// index column is compacted.
    pub compact_after_bytes: u64,
    /// What indexes store.
    pub schema: SchemaOptions,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            commit_interval: Duration::from_millis(250),
            max_rows_per_commit: 2_000,
            extract_batch: 256,
            scan_page: 1_000,
            cache_bytes: 32 << 20,
            max_open_indexes: 256,
            reader_idle: Duration::from_secs(600),
            writer_idle: Duration::from_secs(5),
            audit_interval: Duration::from_secs(30),
            compact_after_bytes: 64 << 20,
            schema: SchemaOptions::default(),
        }
    }
}

/// What one [`SearchService::index_context`] pass did.
#[derive(Clone, Copy, Debug, Default)]
pub struct IndexReport {
    /// Dirty rows consumed.
    pub rows: usize,
    /// Distinct ids extracted.
    pub ids: usize,
    /// Documents added (the rest of `ids` were deletes).
    pub docs: usize,
    /// Commits made.
    pub commits: usize,
    /// Full builds run.
    pub builds: usize,
    /// Documents full builds scanned.
    pub rebuilt: usize,
    /// Times state was found to have moved without a dirty row.
    pub out_of_band: usize,
    /// Time spent in the app's exports.
    pub extract_time: Duration,
    /// Time spent in tantivy (apply + commit).
    pub index_time: Duration,
}

/// The state root and dirty-log head of a context, read as one.
#[derive(Clone, Copy, Debug)]
struct Mark {
    root: [u8; 32],
    head: u64,
}

struct Slot {
    index: Arc<ContextIndex>,
    used: Instant,
    written: Instant,
}

/// The node's search service. Cheap to share: every method takes `&self`.
pub struct SearchService {
    store: Store,
    config: SearchConfig,
    cache: Arc<ChunkCache>,
    indexes: Mutex<HashMap<(ContextKey, String), Slot>>,
    /// The state root each context's indexes last reached, for contexts
    /// indexed since start: what [`observe`](Self::observe) and the audit
    /// compare against.
    roots: Mutex<HashMap<ContextKey, [u8; 32]>>,
    notify_tx: mpsc::UnboundedSender<ContextKey>,
    notify_rx: Mutex<Option<mpsc::UnboundedReceiver<ContextKey>>>,
}

impl std::fmt::Debug for SearchService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchService")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl SearchService {
    /// A service over `store`.
    #[must_use]
    pub fn new(store: Store, config: SearchConfig) -> Arc<Self> {
        let (notify_tx, notify_rx) = mpsc::unbounded_channel();
        Arc::new(Self {
            store,
            cache: ChunkCache::new(config.cache_bytes),
            config,
            indexes: Mutex::default(),
            roots: Mutex::default(),
            notify_tx,
            notify_rx: Mutex::new(Some(notify_rx)),
        })
    }

    /// The shared chunk cache.
    #[must_use]
    pub fn cache(&self) -> &ChunkCache {
        &self.cache
    }

    /// Tell the indexer `context` has new dirty rows (after they committed).
    pub fn notify(&self, context: ContextKey) {
        let _ = self.notify_tx.send(context);
    }

    /// A run of a search-enabled app saw `context` at state root `root`.
    /// Wakes the indexer unless the context's indexes are known to reflect
    /// exactly that root: this is how a context whose state arrived by
    /// snapshot, or moved while search was off, gets (re)built on first use.
    pub fn observe(&self, context: ContextKey, root: [u8; 32]) {
        if !self.reflects(context, root) {
            self.notify(context);
        }
    }

    /// Whether `context`'s indexes are known to reflect state root `root`.
    /// `false` for a context not indexed since this service started.
    #[must_use]
    pub fn reflects(&self, context: ContextKey, root: [u8; 32]) -> bool {
        self.roots
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&context)
            == Some(&root)
    }

    /// Indexes currently open.
    #[must_use]
    pub fn open_indexes(&self) -> usize {
        self.indexes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    fn directory(&self, context: &ContextKey, index: &str) -> RocksDirectory {
        RocksDirectory::new(self.store.clone(), context, index, Arc::clone(&self.cache))
    }

    fn cached(&self, context: &ContextKey, index: &str) -> Option<Arc<ContextIndex>> {
        let mut indexes = self.indexes.lock().unwrap_or_else(PoisonError::into_inner);
        let slot = indexes.get_mut(&(*context, index.to_owned()))?;
        slot.used = Instant::now();
        Some(Arc::clone(&slot.index))
    }

    fn remember(&self, context: ContextKey, index: Arc<ContextIndex>) -> Arc<ContextIndex> {
        let name = index.schema_def().name.clone();
        let now = Instant::now();
        let _ = self
            .indexes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                (context, name),
                Slot {
                    index: Arc::clone(&index),
                    used: now,
                    written: now,
                },
            );
        index
    }

    fn touch_written(&self, context: &ContextKey, index: &str) {
        if let Some(slot) = self
            .indexes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(&(*context, index.to_owned()))
        {
            slot.written = Instant::now();
        }
    }

    /// Open (or create) `def` for `context`, wiping and recreating one built
    /// under another schema. Returns whether it needs a full build.
    ///
    /// # Errors
    /// A tantivy or store failure.
    pub fn open_index(
        &self,
        context: &ContextKey,
        def: &SearchIndexSchema,
    ) -> EyreResult<(Arc<ContextIndex>, bool)> {
        if let Some(index) = self.cached(context, &def.name) {
            if index.schema_def() == def {
                return Ok((index, false));
            }
        }
        let mut dir = self.directory(context, &def.name);
        let (opened, state) = ContextIndex::open(Box::new(dir.clone()), def, self.config.schema)?;
        let (index, state) = match (opened, state) {
            (Some(index), state) => (index, state),
            (None, _) => {
                self.wipe_index(context, &def.name)?;
                dir = self.directory(context, &def.name);
                let (Some(index), state) =
                    ContextIndex::open(Box::new(dir), def, self.config.schema)?
                else {
                    bail!("a freshly wiped index did not open");
                };
                (index, state)
            }
        };
        Ok((
            self.remember(*context, Arc::new(index)),
            state != OpenState::Current,
        ))
    }

    fn wipe_index(&self, context: &ContextKey, index: &str) -> EyreResult<()> {
        let old = self
            .indexes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&(*context, index.to_owned()));
        if let Some(old) = old {
            old.index.close_writer()?;
        }
        let lo = directory::index_prefix(context, index);
        let hi = prefix_upper_bound(&lo);
        self.store.raw_delete_range(Column::SearchIndex, &lo, &hi)
    }

    /// The open index `index` of `context`, opening it from the store if it
    /// was built before. `None` if it never was.
    ///
    /// # Errors
    /// A tantivy or store failure.
    pub fn get_index(
        &self,
        context: &ContextKey,
        index: &str,
    ) -> EyreResult<Option<Arc<ContextIndex>>> {
        if let Some(index) = self.cached(context, index) {
            return Ok(Some(index));
        }
        let dir = self.directory(context, index);
        Ok(ContextIndex::open_existing(Box::new(dir))?
            .map(|opened| self.remember(*context, Arc::new(opened))))
    }

    /// Answer `req` from `context`'s index. The caller names the context from
    /// where the query runs, never from the request: that is the isolation.
    ///
    /// # Errors
    /// An invalid request, or a tantivy failure.
    pub fn search(&self, context: &ContextKey, req: &SearchRequest) -> EyreResult<SearchResponse> {
        let Some(index) = self.get_index(context, &req.index)? else {
            // Not built yet (or no such index): nothing to find, not an error a
            // view should have to special-case.
            return Ok(SearchResponse::default());
        };
        index.search(req)
    }

    /// Drop every index and dirty row of `context`, and forget it.
    ///
    /// # Errors
    /// A store failure.
    pub fn delete_context(&self, context: &ContextKey) -> EyreResult<()> {
        let doomed: Vec<Arc<ContextIndex>> = {
            let mut indexes = self.indexes.lock().unwrap_or_else(PoisonError::into_inner);
            let keys: Vec<_> = indexes
                .keys()
                .filter(|(c, _)| c == context)
                .cloned()
                .collect();
            keys.into_iter()
                .filter_map(|k| indexes.remove(&k))
                .map(|slot| slot.index)
                .collect()
        };
        for index in doomed {
            index.close_writer()?;
        }
        let _ = self
            .roots
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(context);
        directory::delete_context(&self.store, &self.cache, context)
    }

    async fn mark(&self, source: &dyn ContextSource, context: ContextKey) -> EyreResult<Mark> {
        let _guard = source.lock(context).await?;
        Ok(Mark {
            root: source.state_root(context)?,
            head: dirty::head(&self.store, &context)?,
        })
    }

    async fn full_build(
        &self,
        source: &dyn ContextSource,
        context: ContextKey,
        index: &ContextIndex,
        report: &mut IndexReport,
    ) -> EyreResult<()> {
        // Everything logged so far is covered by the scan below, and the scan
        // starts from `mark.root`. What lands while it runs has a higher seq
        // and chains from that root, so it is replayed afterwards; what moves
        // state without a row breaks the chain or the final root check, and
        // brings the next build.
        let mark = self.mark(source, context).await?;
        index.clear()?;
        let name = index.schema_def().name.clone();
        let mut offset = 0;
        loop {
            let t = Instant::now();
            let page = source
                .scan(context, &name, offset, self.config.scan_page)
                .await?;
            report.extract_time += t.elapsed();
            let t = Instant::now();
            report.rebuilt += index.apply(page.docs.iter().map(|d| (&d.id, Some(d))))?;
            report.index_time += t.elapsed();
            match page.next {
                Some(next) if next > offset => offset = next,
                _ => break,
            }
        }
        let t = Instant::now();
        index.commit(mark.head, mark.root)?;
        report.index_time += t.elapsed();
        report.commits += 1;
        report.builds += 1;
        Ok(())
    }

    /// Bring one index up to date: replay the dirty rows that chain from the
    /// root it reflects, and rebuild it from state when the chain breaks or
    /// state moved past its last row.
    async fn sync_index(
        &self,
        source: &dyn ContextSource,
        context: ContextKey,
        index: &ContextIndex,
        mut needs_build: bool,
        report: &mut IndexReport,
    ) -> EyreResult<()> {
        let name = index.schema_def().name.clone();
        let mut builds = 0;
        loop {
            if needs_build {
                if builds == MAX_BUILDS_PER_PASS {
                    bail!("state of the context keeps moving under a full build of {name:?}");
                }
                builds += 1;
                self.full_build(source, context, index, report).await?;
                self.touch_written(&context, &name);
                needs_build = false;
            }

            let from = index.committed_seq();
            let rows =
                dirty::read_after(&self.store, &context, from, self.config.max_rows_per_commit)?;
            if rows.is_empty() {
                let mark = self.mark(source, context).await?;
                if mark.head > from {
                    // Rows landed after the read above.
                    continue;
                }
                if mark.root != index.committed_root() {
                    report.out_of_band += 1;
                    needs_build = true;
                    continue;
                }
                return Ok(());
            }

            let mut root = index.committed_root();
            if rows.iter().any(|row| {
                let linked = row.before == root;
                root = row.after;
                !linked
            }) {
                report.out_of_band += 1;
                needs_build = true;
                continue;
            }
            report.rows += rows.len();

            let mut seen = BTreeSet::new();
            let ids: Vec<[u8; 32]> = rows
                .iter()
                .flat_map(|r| r.ids.iter().copied())
                .filter(|id| seen.insert(*id))
                .collect();
            report.ids += ids.len();
            for batch in ids.chunks(self.config.extract_batch) {
                let t = Instant::now();
                let docs = source.extract(context, &name, batch.to_vec()).await?;
                report.extract_time += t.elapsed();
                if docs.len() != batch.len() {
                    bail!(
                        "extract answered {} documents for {} ids",
                        docs.len(),
                        batch.len()
                    );
                }
                report.docs += docs.iter().flatten().count();
                let t = Instant::now();
                let _ = index.apply(batch.iter().zip(docs.iter().map(Option::as_ref)))?;
                report.index_time += t.elapsed();
            }
            let last = rows.last().map_or(from, |r| r.seq);
            let t = Instant::now();
            index.commit(last, root)?;
            report.index_time += t.elapsed();
            report.commits += 1;
            self.touch_written(&context, &name);
        }
    }

    /// Bring every index of `context` up to date with its state, building an
    /// index from scratch when it is new, its schema changed, or state moved
    /// without going through the dirty log.
    ///
    /// # Errors
    /// An export, tantivy or store failure. The dirty log is kept past the
    /// last successful commit, so a retry picks up there.
    pub async fn index_context(
        &self,
        source: &dyn ContextSource,
        context: ContextKey,
    ) -> EyreResult<IndexReport> {
        let mut report = IndexReport::default();
        let t = Instant::now();
        let defs = source.schema(context).await?;
        report.extract_time += t.elapsed();
        let Some(defs) = defs.filter(|d| !d.is_empty()) else {
            // The app no longer declares an index (an upgrade dropped it):
            // nothing derived from this context's state is wanted any more.
            self.delete_context(&context)?;
            return Ok(report);
        };

        for name in directory::index_names(&self.store, &context)? {
            if !defs.iter().any(|def| def.name == name) {
                self.wipe_index(&context, &name)?;
            }
        }

        let mut floor = u64::MAX;
        let mut roots = BTreeSet::new();
        for def in &defs {
            let (index, needs_build) = self.open_index(&context, def)?;
            self.sync_index(source, context, &index, needs_build, &mut report)
                .await?;
            floor = floor.min(index.committed_seq());
            let _ = roots.insert(index.committed_root());
        }
        if floor > 0 {
            dirty::trim_through(&self.store, &context, floor)?;
        }
        // Every index ended its pass on the same root unless state moved
        // between them; then the next observation brings another pass.
        let mut known = self.roots.lock().unwrap_or_else(PoisonError::into_inner);
        match (roots.len(), roots.first()) {
            (1, Some(root)) => {
                let _ = known.insert(context, *root);
            }
            _ => {
                let _ = known.remove(&context);
            }
        }
        Ok(report)
    }

    /// Close the writer of every open index (between bursts, to give back
    /// their arenas and threads). Readers stay open.
    ///
    /// # Errors
    /// A failure while waiting for merges.
    pub fn close_writers(&self) -> EyreResult<()> {
        self.close_writers_idle_for(Duration::ZERO)
    }

    fn close_writers_idle_for(&self, idle: Duration) -> EyreResult<()> {
        let now = Instant::now();
        let idle: Vec<Arc<ContextIndex>> = self
            .indexes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .filter(|slot| now.duration_since(slot.written) >= idle && slot.index.has_writer())
            .map(|slot| Arc::clone(&slot.index))
            .collect();
        for index in idle {
            index.close_writer()?;
        }
        Ok(())
    }

    /// Close indexes no query or pass used within `reader_idle`, then the
    /// least recently used ones past `max_open_indexes`. An index some query
    /// still holds is never closed under it.
    ///
    /// # Errors
    /// A failure while waiting for a closing writer's merges.
    pub fn evict_idle(&self) -> EyreResult<usize> {
        let now = Instant::now();
        let evicted: Vec<Arc<ContextIndex>> = {
            let mut indexes = self.indexes.lock().unwrap_or_else(PoisonError::into_inner);
            let mut idle: Vec<((ContextKey, String), Instant)> = indexes
                .iter()
                .filter(|(_, slot)| Arc::strong_count(&slot.index) == 1)
                .map(|(key, slot)| (key.clone(), slot.used))
                .collect();
            idle.sort_by_key(|(_, used)| *used);
            let mut over = indexes.len().saturating_sub(self.config.max_open_indexes);
            let mut out = Vec::new();
            for (key, used) in idle {
                let expired = now.duration_since(used) >= self.config.reader_idle;
                if !expired && over == 0 {
                    break;
                }
                over = over.saturating_sub(1);
                if let Some(slot) = indexes.remove(&key) {
                    out.push(slot.index);
                }
            }
            out
        };
        let n = evicted.len();
        for index in evicted {
            index.close_writer()?;
        }
        Ok(n)
    }

    /// Compact the index column's slice of every context that deleted at
    /// least `compact_after_bytes` of index files since its last compaction.
    /// Returns the contexts compacted.
    ///
    /// # Errors
    /// A store failure.
    pub fn compact(&self) -> EyreResult<usize> {
        let due = self
            .cache
            .take_removed_over(self.config.compact_after_bytes);
        for (context, bytes) in &due {
            let lo = directory::context_prefix(context);
            let hi = prefix_upper_bound(&lo);
            let t = Instant::now();
            self.store.raw_compact_range(Column::SearchIndex, &lo, &hi)?;
            debug!(freed = bytes, took = ?t.elapsed(), "compacted a context's search index");
        }
        Ok(due.len())
    }

    /// The contexts indexed since start whose state root is no longer the one
    /// their indexes reached.
    fn audit(&self, source: &dyn ContextSource) -> Vec<ContextKey> {
        let known: Vec<(ContextKey, [u8; 32])> = self
            .roots
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(c, r)| (*c, *r))
            .collect();
        known
            .into_iter()
            .filter(|(context, root)| match source.state_root(*context) {
                Ok(now) => now != *root,
                Err(err) => {
                    warn!(%err, "could not read a context's state root for the search audit");
                    false
                }
            })
            .map(|(context, _)| context)
            .collect()
    }

    /// Close what went idle and compact what shrank. Awaited between passes,
    /// never beside one: closing a writer drops whatever it has not committed.
    async fn maintain(self: &Arc<Self>) {
        if self.open_indexes() == 0 && !self.cache.has_removed() {
            return;
        }
        let service = Arc::clone(self);
        // Closing writers waits for merges and compaction is a blocking
        // store call: neither belongs on the async runtime's workers.
        let done = tokio::task::spawn_blocking(move || {
            if let Err(err) = service.close_writers_idle_for(service.config.writer_idle) {
                warn!(%err, "could not close an idle search writer");
            }
            match service.evict_idle() {
                Ok(0) => {}
                Ok(n) => debug!(evicted = n, "closed idle search indexes"),
                Err(err) => warn!(%err, "could not close an idle search index"),
            }
            if let Err(err) = service.compact() {
                warn!(%err, "could not compact the search index column");
            }
        })
        .await;
        if let Err(err) = done {
            warn!(%err, "search maintenance task failed");
        }
    }

    /// The indexer: every `commit_interval`, bring each context that was
    /// notified (or, at start, still has a backlog; or failed the audit) up to
    /// date, then close what went idle and compact what shrank. Runs until the
    /// service's last sender drops, which is never while the service lives,
    /// so spawn it.
    pub async fn run_indexer(self: Arc<Self>, source: Arc<dyn ContextSource>) {
        let Some(mut rx) = self
            .notify_rx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        else {
            warn!("search indexer started twice; the second one exits");
            return;
        };
        let mut pending: BTreeSet<ContextKey> = match dirty::contexts_with_rows(&self.store) {
            Ok(backlog) => backlog.into_iter().collect(),
            Err(err) => {
                warn!(%err, "could not read the search backlog");
                BTreeSet::new()
            }
        };
        info!(backlog = pending.len(), "search indexer started");
        let mut tick = tokio::time::interval(self.config.commit_interval);
        let mut audit = tokio::time::interval(self.config.audit_interval);
        loop {
            tokio::select! {
                got = rx.recv() => match got {
                    Some(context) => { let _ = pending.insert(context); }
                    None => return,
                },
                _ = audit.tick() => pending.extend(self.audit(&*source)),
                _ = tick.tick() => {
                    for context in std::mem::take(&mut pending) {
                        match self.index_context(&*source, context).await {
                            Ok(report) => debug!(?report, "search indexer pass"),
                            Err(err) => {
                                warn!(%err, "search indexing failed; retrying next tick");
                                let _ = pending.insert(context);
                            }
                        }
                    }
                    self.maintain().await;
                }
            }
        }
    }
}
