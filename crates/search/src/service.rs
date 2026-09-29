//! The node's search service: open indexes, the indexer loop, the query entry.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use calimero_primitives::search::{
    ExtractResponse, ScanResponse, SearchIndexSchema, SearchRequest, SearchResponse,
};
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::directory::{self, ChunkCache, RocksDirectory};
use crate::dirty;
use crate::index::{ContextIndex, OpenState, SchemaOptions};

/// A context id, as the store keys it.
pub type ContextKey = [u8; 32];

/// What the indexer asks of the app a context runs. The node implements it by
/// running the app's search exports read-only against current state.
#[async_trait]
pub trait Extractor: Send + Sync + 'static {
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
}

/// Service knobs.
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
    /// Documents a full build scanned (0 when none ran).
    pub rebuilt: usize,
    /// Time spent in the extractor.
    pub extract_time: Duration,
    /// Time spent in tantivy (apply + commit).
    pub index_time: Duration,
}

/// The node's search service. Cheap to share: every method takes `&self`.
pub struct SearchService {
    store: Store,
    config: SearchConfig,
    cache: Arc<ChunkCache>,
    indexes: Mutex<HashMap<(ContextKey, String), Arc<ContextIndex>>>,
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

    fn directory(&self, context: &ContextKey, index: &str) -> RocksDirectory {
        RocksDirectory::new(self.store.clone(), context, index, Arc::clone(&self.cache))
    }

    fn cached(&self, context: &ContextKey, index: &str) -> Option<Arc<ContextIndex>> {
        self.indexes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&(*context, index.to_owned()))
            .cloned()
    }

    fn remember(&self, context: ContextKey, index: Arc<ContextIndex>) -> Arc<ContextIndex> {
        let name = index.schema_def().name.clone();
        let _ = self
            .indexes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert((context, name), Arc::clone(&index));
        index
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
        if let Some(old) = self
            .indexes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&(*context, index.to_owned()))
        {
            old.close_writer()?;
        }
        let lo = directory::index_prefix(context, index);
        let hi = calimero_primitives::utils::prefix_upper_bound(&lo);
        self.store
            .raw_delete_range(calimero_store::db::Column::SearchIndex, &lo, &hi)
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
    /// An unknown index, an invalid request, or a tantivy failure.
    pub fn search(&self, context: &ContextKey, req: &SearchRequest) -> EyreResult<SearchResponse> {
        let Some(index) = self.get_index(context, &req.index)? else {
            // Not built yet (or no such index): nothing to find, not an error a
            // view should have to special-case.
            return Ok(SearchResponse::default());
        };
        index.search(req)
    }

    /// Drop every index and dirty row of `context`.
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
                .collect()
        };
        for index in doomed {
            index.close_writer()?;
        }
        directory::delete_context(&self.store, &self.cache, context)
    }

    /// Close the writer of every open index (between bursts, to give back
    /// their arenas and threads). Readers stay open.
    ///
    /// # Errors
    /// A failure while waiting for merges.
    pub fn close_writers(&self) -> EyreResult<()> {
        let all: Vec<Arc<ContextIndex>> = self
            .indexes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        for index in all {
            index.close_writer()?;
        }
        Ok(())
    }

    async fn full_build(
        &self,
        extractor: &dyn Extractor,
        context: ContextKey,
        index: &ContextIndex,
        report: &mut IndexReport,
    ) -> EyreResult<()> {
        // Everything logged so far is covered by the scan below; what lands
        // while it runs has a higher seq and is replayed afterwards.
        let covered = dirty::last_seq(&self.store, &context)?.unwrap_or(0);
        index.clear()?;
        let name = index.schema_def().name.clone();
        let mut offset = 0;
        loop {
            let t = Instant::now();
            let page = extractor
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
        index.commit(covered)?;
        report.index_time += t.elapsed();
        report.commits += 1;
        Ok(())
    }

    /// Bring every index of `context` up to date with its dirty log, building
    /// any index from scratch first if it has to be.
    ///
    /// # Errors
    /// An extractor, tantivy or store failure; the dirty log is left as it
    /// was past the last successful commit, so a retry picks up there.
    pub async fn index_context(
        &self,
        extractor: &dyn Extractor,
        context: ContextKey,
    ) -> EyreResult<IndexReport> {
        let mut report = IndexReport::default();
        let t = Instant::now();
        let defs = extractor.schema(context).await?;
        report.extract_time += t.elapsed();
        let Some(defs) = defs.filter(|d| !d.is_empty()) else {
            if let Some(last) = dirty::last_seq(&self.store, &context)? {
                dirty::trim_through(&self.store, &context, last)?;
            }
            return Ok(report);
        };

        let mut indexes = Vec::with_capacity(defs.len());
        for def in &defs {
            let (index, needs_build) = self.open_index(&context, def)?;
            if needs_build {
                self.full_build(extractor, context, &index, &mut report)
                    .await?;
            }
            indexes.push(index);
        }

        loop {
            let from = indexes.iter().map(|i| i.committed_seq()).min().unwrap_or(0);
            // Whatever every index already covers (a full build's scan, or a
            // commit whose trim a crash lost) goes first.
            if from > 0 {
                dirty::trim_through(&self.store, &context, from)?;
            }
            let rows =
                dirty::read_after(&self.store, &context, from, self.config.max_rows_per_commit)?;
            let Some(last) = rows.last().map(|r| r.seq) else {
                break;
            };
            report.rows += rows.len();

            for index in &indexes {
                let mut seen = BTreeSet::new();
                let ids: Vec<[u8; 32]> = rows
                    .iter()
                    .filter(|r| r.seq > index.committed_seq())
                    .flat_map(|r| r.ids.iter().copied())
                    .filter(|id| seen.insert(*id))
                    .collect();
                report.ids += ids.len();
                let name = index.schema_def().name.clone();
                for batch in ids.chunks(self.config.extract_batch) {
                    let t = Instant::now();
                    let docs = extractor.extract(context, &name, batch.to_vec()).await?;
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
                let t = Instant::now();
                index.commit(last)?;
                report.index_time += t.elapsed();
                report.commits += 1;
            }
            dirty::trim_through(&self.store, &context, last)?;
        }
        Ok(report)
    }

    /// The indexer: every `commit_interval`, bring each context that was
    /// notified (or, at start, still has a backlog) up to date. Runs until the
    /// service's last sender drops, which is never while the service lives, so
    /// spawn it.
    pub async fn run_indexer(self: Arc<Self>, extractor: Arc<dyn Extractor>) {
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
        let mut tick = tokio::time::interval(self.config.commit_interval);
        loop {
            tokio::select! {
                got = rx.recv() => match got {
                    Some(context) => { let _ = pending.insert(context); }
                    None => return,
                },
                _ = tick.tick() => {
                    for context in std::mem::take(&mut pending) {
                        match self.index_context(&*extractor, context).await {
                            Ok(report) => debug!(?report, "search indexer pass"),
                            Err(err) => {
                                warn!(%err, "search indexing failed; retrying next tick");
                                let _ = pending.insert(context);
                            }
                        }
                    }
                }
            }
        }
    }
}
