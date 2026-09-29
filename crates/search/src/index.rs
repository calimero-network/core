//! One tantivy index of one context: schema, writes, and queries.

use std::collections::HashMap;
use std::ops::Bound;
use std::sync::{Mutex, PoisonError};

use calimero_primitives::search::{
    SearchDoc, SearchFieldKind, SearchFilter, SearchHit, SearchIndexSchema, SearchMode,
    SearchRequest, SearchResponse, SearchValue,
};
use eyre::{bail, eyre, Result as EyreResult};
use serde::{Deserialize, Serialize};
use tantivy::collector::{Count, TopDocs};
use tantivy::indexer::IndexWriterOptions;
use tantivy::query::{
    AllQuery, BooleanQuery, BoostQuery, ConstScoreQuery, FuzzyTermQuery, Occur, PhraseQuery, Query,
    RangeQuery, TermQuery,
};
use tantivy::schema::{
    Field, IndexRecordOption, Schema, TextFieldIndexing, TextOptions, Value, FAST, INDEXED, STORED,
    STRING,
};
use tantivy::snippet::SnippetGenerator;
use tantivy::{
    Directory, Index, IndexReader, IndexSettings, IndexWriter, ReloadPolicy, TantivyDocument, Term,
};

use crate::tokenize::{self, TOKENIZER_VERSION};

/// Largest page a query may ask for.
pub const MAX_LIMIT: u32 = 100;

/// Deepest a cursor may page.
pub const MAX_CURSOR: u32 = 10_000;

/// Longest query text accepted, in bytes.
pub const MAX_QUERY_LEN: usize = 256;

/// Writer arena per open index: tantivy's floor.
pub const WRITER_MEMORY: usize = 15_000_000;

/// Layout of [`CommitMeta`]. An index committed under another format (or
/// whose payload does not decode) opens as [`OpenState::Stale`] and is rebuilt
/// from state, which is the whole migration story for the index: it is
/// derived data.
pub const FORMAT_VERSION: u32 = 1;

/// Knobs that change what an index stores (the benchmark flips them).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SchemaOptions {
    /// Store text fields in the doc store, for snippets.
    pub store_text: bool,
    /// Honour `infix: true` (index trigrams).
    pub trigrams: bool,
}

impl Default for SchemaOptions {
    fn default() -> Self {
        Self {
            store_text: true,
            trigrams: true,
        }
    }
}

/// What an index records in every commit's payload.
#[derive(Debug, Deserialize, PartialEq, Serialize)]
struct CommitMeta {
    format: u32,
    /// Every dirty-log row up to and including this seq is in the index.
    seq: u64,
    /// The context state root the index reflects: the `after` of the row at
    /// `seq`, or the root a full build started from.
    root: [u8; 32],
    tokenizer: u32,
    options: SchemaOptions,
    schema: SearchIndexSchema,
}

/// How [`ContextIndex::open`] found the index.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenState {
    /// It was current and is ready.
    Current,
    /// There was none; it is empty and needs a full build from state.
    Created,
    /// There was one under another schema, tokenizer or options; the caller
    /// must wipe it and open again.
    Stale,
}

struct TextField {
    words: Field,
    tri: Option<Field>,
    boost: f32,
}

struct Fields {
    id: Field,
    text: Vec<TextField>,
    by_name: HashMap<String, (SearchFieldKind, Field, Option<Field>)>,
}

fn build_schema(def: &SearchIndexSchema, opts: SchemaOptions) -> Schema {
    let mut b = Schema::builder();
    let _ = b.add_bytes_field("_id", INDEXED | STORED);
    for field in &def.fields {
        match &field.kind {
            SearchFieldKind::Text { infix, .. } => {
                let indexing = TextFieldIndexing::default()
                    .set_tokenizer(tokenize::WORDS)
                    .set_index_option(IndexRecordOption::WithFreqsAndPositions);
                let mut options = TextOptions::default().set_indexing_options(indexing);
                if opts.store_text {
                    options = options.set_stored();
                }
                let _ = b.add_text_field(&field.name, options);
                if *infix && opts.trigrams {
                    let tri = TextFieldIndexing::default()
                        .set_tokenizer(tokenize::TRIGRAMS)
                        .set_index_option(IndexRecordOption::WithFreqsAndPositions);
                    let _ = b.add_text_field(
                        &format!("{}._tri", field.name),
                        TextOptions::default().set_indexing_options(tri),
                    );
                }
            }
            SearchFieldKind::Keyword => {
                let _ = b.add_text_field(&field.name, STRING | FAST);
            }
            SearchFieldKind::U64 => {
                let _ = b.add_u64_field(&field.name, INDEXED | FAST);
            }
        }
    }
    b.build()
}

fn resolve_fields(schema: &Schema, def: &SearchIndexSchema) -> EyreResult<Fields> {
    let id = schema.get_field("_id")?;
    let mut text = Vec::new();
    let mut by_name = HashMap::new();
    for field in &def.fields {
        let main = schema.get_field(&field.name)?;
        let tri = match field.kind {
            SearchFieldKind::Text { .. } => schema.get_field(&format!("{}._tri", field.name)).ok(),
            _ => None,
        };
        if let SearchFieldKind::Text { weight, .. } = field.kind {
            text.push(TextField {
                words: main,
                tri,
                boost: weight as f32 / 100.0,
            });
        }
        let _ = by_name.insert(field.name.clone(), (field.kind.clone(), main, tri));
    }
    Ok(Fields { id, text, by_name })
}

/// One `(context, index)` pair's tantivy index.
pub struct ContextIndex {
    def: SearchIndexSchema,
    opts: SchemaOptions,
    index: Index,
    fields: Fields,
    reader: IndexReader,
    writer: Mutex<Option<IndexWriter<TantivyDocument>>>,
    committed: Mutex<(u64, [u8; 32])>,
}

impl ContextIndex {
    /// Open `def` in `dir`, creating it if absent.
    ///
    /// # Errors
    /// A tantivy or store failure.
    pub fn open(
        dir: Box<dyn Directory>,
        def: &SearchIndexSchema,
        opts: SchemaOptions,
    ) -> EyreResult<(Option<Self>, OpenState)> {
        let exists = Index::exists(&*dir)?;
        let (index, state) = if exists {
            let index = Index::open(dir)?;
            let meta: Option<CommitMeta> = index
                .load_metas()?
                .payload
                .as_deref()
                .and_then(|p| serde_json::from_str(p).ok());
            let current = meta.as_ref().is_some_and(|m| {
                m.format == FORMAT_VERSION
                    && m.tokenizer == TOKENIZER_VERSION
                    && m.options == opts
                    && m.schema == *def
            });
            if !current {
                return Ok((None, OpenState::Stale));
            }
            (index, OpenState::Current)
        } else {
            let index = Index::create(dir, build_schema(def, opts), IndexSettings::default())?;
            (index, OpenState::Created)
        };
        tokenize::register(&index);
        let fields = resolve_fields(&index.schema(), def)?;
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;
        let this = Self {
            def: def.clone(),
            opts,
            index,
            fields,
            reader,
            writer: Mutex::new(None),
            committed: Mutex::new((0, [0; 32])),
        };
        if state == OpenState::Created {
            // Record the schema right away, so the index can be reopened for a
            // query before anything was ever written to it.
            this.commit(0, [0; 32])?;
        } else if let Some(meta) = this.read_payload()? {
            *this.committed.lock().unwrap_or_else(PoisonError::into_inner) = (meta.seq, meta.root);
        }
        Ok((Some(this), state))
    }

    /// Open whatever index `dir` holds, under the schema its last commit
    /// recorded — the query path's open, which has no app to ask.
    ///
    /// # Errors
    /// A tantivy or store failure.
    pub fn open_existing(dir: Box<dyn Directory>) -> EyreResult<Option<Self>> {
        if !Index::exists(&*dir)? {
            return Ok(None);
        }
        let index = Index::open(dir.box_clone())?;
        let Some(meta) = index
            .load_metas()?
            .payload
            .as_deref()
            .and_then(|p| serde_json::from_str::<CommitMeta>(p).ok())
        else {
            return Ok(None);
        };
        drop(index);
        match Self::open(dir, &meta.schema, meta.options)? {
            (Some(this), OpenState::Current) => Ok(Some(this)),
            _ => Ok(None),
        }
    }

    fn read_payload(&self) -> EyreResult<Option<CommitMeta>> {
        Ok(self
            .index
            .load_metas()?
            .payload
            .as_deref()
            .and_then(|p| serde_json::from_str(p).ok()))
    }

    /// The schema this index was opened under.
    #[must_use]
    pub fn schema_def(&self) -> &SearchIndexSchema {
        &self.def
    }

    /// The dirty-log seq the last commit covers.
    #[must_use]
    pub fn committed_seq(&self) -> u64 {
        self.committed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .0
    }

    /// The context state root the last commit reflects.
    #[must_use]
    pub fn committed_root(&self) -> [u8; 32] {
        self.committed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .1
    }

    fn with_writer<T>(
        &self,
        f: impl FnOnce(&mut IndexWriter<TantivyDocument>) -> EyreResult<T>,
    ) -> EyreResult<T> {
        let mut guard = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        if guard.is_none() {
            let options = IndexWriterOptions::builder()
                .memory_budget_per_thread(WRITER_MEMORY)
                .num_worker_threads(1)
                .num_merge_threads(1)
                .build();
            *guard = Some(self.index.writer_with_options(options)?);
        }
        let writer = guard.as_mut().ok_or_else(|| eyre!("writer vanished"))?;
        f(writer)
    }

    /// Drop the writer (its arena and threads) until the next write needs it.
    ///
    /// # Errors
    /// A failure while waiting for its merges.
    pub fn close_writer(&self) -> EyreResult<()> {
        let writer = self
            .writer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(writer) = writer {
            writer.wait_merging_threads()?;
            // The merges just replaced segments the current searcher still
            // holds; move it onto the merged ones so the old files can go.
            self.reader.reload()?;
        }
        Ok(())
    }

    /// Whether a writer is open.
    #[must_use]
    pub fn has_writer(&self) -> bool {
        self.writer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    fn id_term(&self, id: &[u8; 32]) -> Term {
        Term::from_field_bytes(self.fields.id, id)
    }

    fn to_document(&self, doc: &SearchDoc) -> TantivyDocument {
        let mut out = TantivyDocument::default();
        out.add_bytes(self.fields.id, &doc.id[..]);
        for (name, value) in &doc.fields {
            let Some((kind, main, tri)) = self.fields.by_name.get(name) else {
                continue;
            };
            match (kind, value) {
                (SearchFieldKind::Text { .. }, SearchValue::Str(s)) => {
                    out.add_text(*main, s);
                    if let Some(tri) = tri {
                        out.add_text(*tri, s);
                    }
                }
                (SearchFieldKind::Keyword, SearchValue::Str(s)) => out.add_text(*main, s),
                (SearchFieldKind::U64, SearchValue::U64(v)) => out.add_u64(*main, *v),
                _ => {}
            }
        }
        out
    }

    /// Replace the document of every id: delete by id, then add the new one if
    /// there is one. Idempotent, so a replayed id is harmless.
    ///
    /// # Errors
    /// A tantivy failure.
    pub fn apply<'a>(
        &self,
        changes: impl IntoIterator<Item = (&'a [u8; 32], Option<&'a SearchDoc>)>,
    ) -> EyreResult<usize> {
        self.with_writer(|writer| {
            let mut n = 0;
            for (id, doc) in changes {
                let _ = writer.delete_term(self.id_term(id));
                if let Some(doc) = doc {
                    let _ = writer.add_document(self.to_document(doc))?;
                }
                n += 1;
            }
            Ok(n)
        })
    }

    /// Drop every document (a full rebuild's first step).
    ///
    /// # Errors
    /// A tantivy failure.
    pub fn clear(&self) -> EyreResult<()> {
        self.with_writer(|writer| {
            let _ = writer.delete_all_documents()?;
            Ok(())
        })
    }

    /// Commit, recording that it covers every dirty row through `seq` and
    /// reflects state root `root`, and make it searchable.
    ///
    /// # Errors
    /// A tantivy failure.
    pub fn commit(&self, seq: u64, root: [u8; 32]) -> EyreResult<()> {
        let payload = serde_json::to_string(&CommitMeta {
            format: FORMAT_VERSION,
            seq,
            root,
            tokenizer: TOKENIZER_VERSION,
            options: self.opts,
            schema: self.def.clone(),
        })?;
        self.with_writer(|writer| {
            let mut prepared = writer.prepare_commit()?;
            prepared.set_payload(&payload);
            let _ = prepared.commit()?;
            Ok(())
        })?;
        self.reader.reload()?;
        *self.committed.lock().unwrap_or_else(PoisonError::into_inner) = (seq, root);
        Ok(())
    }

    /// Documents in the index as of the last commit.
    #[must_use]
    pub fn num_docs(&self) -> u64 {
        self.reader.searcher().num_docs()
    }

    fn filter_query(&self, filter: &SearchFilter) -> EyreResult<Box<dyn Query>> {
        let field = |name: &str| {
            self.fields
                .by_name
                .get(name)
                .ok_or_else(|| eyre!("no field {name:?} in index {:?}", self.def.name))
        };
        let q: Box<dyn Query> = match filter {
            SearchFilter::Eq { field: name, value } => {
                let (kind, f, _) = field(name)?;
                if *kind != SearchFieldKind::Keyword {
                    bail!("{name:?} is not a keyword field");
                }
                Box::new(TermQuery::new(
                    Term::from_field_text(*f, value),
                    IndexRecordOption::Basic,
                ))
            }
            SearchFilter::Range {
                field: name,
                min,
                max,
            } => {
                let (kind, f, _) = field(name)?;
                if *kind != SearchFieldKind::U64 {
                    bail!("{name:?} is not a u64 field");
                }
                Box::new(RangeQuery::new(
                    Bound::Included(Term::from_field_u64(*f, *min)),
                    Bound::Included(Term::from_field_u64(*f, *max)),
                ))
            }
        };
        Ok(Box::new(ConstScoreQuery::new(q, 0.0)))
    }

    fn text_query(&self, req: &SearchRequest) -> EyreResult<Option<Box<dyn Query>>> {
        if req.mode == SearchMode::Substring {
            let chars = tokenize::trigram_text(&req.query);
            if chars.len() < 3 {
                bail!("a substring query needs at least 3 characters");
            }
            let grams: Vec<String> = chars.windows(3).map(|w| w.iter().collect()).collect();
            let mut per_field: Vec<(Occur, Box<dyn Query>)> = Vec::new();
            for f in &self.fields.text {
                let Some(tri) = f.tri else { continue };
                let q: Box<dyn Query> = if grams.len() == 1 {
                    Box::new(TermQuery::new(
                        Term::from_field_text(tri, &grams[0]),
                        IndexRecordOption::WithFreqs,
                    ))
                } else {
                    Box::new(PhraseQuery::new(
                        grams
                            .iter()
                            .map(|g| Term::from_field_text(tri, g))
                            .collect(),
                    ))
                };
                per_field.push((Occur::Should, Box::new(BoostQuery::new(q, f.boost))));
            }
            if per_field.is_empty() {
                bail!("index {:?} has no infix field", self.def.name);
            }
            return Ok(Some(Box::new(BooleanQuery::new(per_field))));
        }

        let terms: Vec<String> = tokenize::words(&req.query)
            .into_iter()
            .map(|t| t.text)
            .collect();
        if terms.is_empty() {
            return Ok(None);
        }
        let last = terms.len() - 1;
        let mut must: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        for (i, text) in terms.iter().enumerate() {
            let mut per_field: Vec<(Occur, Box<dyn Query>)> = Vec::new();
            for f in &self.fields.text {
                let term = Term::from_field_text(f.words, text);
                let q: Box<dyn Query> = match req.mode {
                    SearchMode::Prefix if i == last => {
                        Box::new(FuzzyTermQuery::new_prefix(term, 0, false))
                    }
                    SearchMode::Fuzzy => Box::new(FuzzyTermQuery::new(term, 1, true)),
                    _ => Box::new(TermQuery::new(term, IndexRecordOption::WithFreqs)),
                };
                per_field.push((Occur::Should, Box::new(BoostQuery::new(q, f.boost))));
            }
            must.push((Occur::Must, Box::new(BooleanQuery::new(per_field))));
        }
        Ok(Some(Box::new(BooleanQuery::new(must))))
    }

    /// Run `req` against the last commit.
    ///
    /// # Errors
    /// An invalid request (unknown field, oversized page) or a tantivy failure.
    pub fn search(&self, req: &SearchRequest) -> EyreResult<SearchResponse> {
        if req.query.len() > MAX_QUERY_LEN {
            bail!("query longer than {MAX_QUERY_LEN} bytes");
        }
        let limit = req.limit.clamp(1, MAX_LIMIT) as usize;
        let cursor = req.cursor.min(MAX_CURSOR) as usize;

        let text = self.text_query(req)?;
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        if let Some(text) = &text {
            clauses.push((Occur::Must, text.box_clone()));
        }
        for filter in &req.filters {
            clauses.push((Occur::Must, self.filter_query(filter)?));
        }
        let query: Box<dyn Query> = if clauses.is_empty() {
            Box::new(AllQuery)
        } else {
            Box::new(BooleanQuery::new(clauses))
        };

        let searcher = self.reader.searcher();
        let (top, total) = searcher.search(
            &query,
            &(
                TopDocs::with_limit(limit)
                    .and_offset(cursor)
                    .order_by_score(),
                Count,
            ),
        )?;

        let snippets = match (&text, self.opts.store_text, self.fields.text.first()) {
            (Some(text), true, Some(f)) if req.mode != SearchMode::Substring => {
                let mut g = SnippetGenerator::create(&searcher, &**text, f.words)?;
                g.set_max_num_chars(120);
                Some((g, f.words))
            }
            _ => None,
        };

        let mut hits = Vec::with_capacity(top.len());
        for (score, addr) in top {
            let doc: TantivyDocument = searcher.doc(addr)?;
            let id: [u8; 32] = doc
                .get_first(self.fields.id)
                .and_then(|v| v.as_bytes())
                .and_then(|b| b.try_into().ok())
                .ok_or_else(|| eyre!("a document without an id"))?;
            let snippet = match &snippets {
                Some((g, _)) => g.snippet_from_doc(&doc).to_html(),
                None => String::new(),
            };
            hits.push(SearchHit { id, score, snippet });
        }
        let next = cursor + hits.len();
        Ok(SearchResponse {
            next_cursor: (hits.len() == limit && next < total)
                .then(|| u32::try_from(next).ok())
                .flatten(),
            total: total as u64,
            hits,
        })
    }
}
