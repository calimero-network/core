//! Per-app full-text search, derived from state and never replicated.
//!
//! - [`index`]: one index — schema from the app's declaration, idempotent
//!   delete-then-add writes, BM25 queries with filters. The node opens it over
//!   any tantivy `Directory` (in memory, `component::search::ComponentSearch`)
//!   and keeps it up to date from the records each entry touched.
//! - [`tokenize`]: Unicode words, accent/case folding, CJK bigrams, trigrams.

pub mod index;
pub mod tokenize;
