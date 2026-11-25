//! Per-app full-text search, derived from state and never replicated.
//!
//! - [`index`]: one index — schema from the app's declaration, idempotent
//!   delete-then-add writes, BM25 queries with filters. The node opens it over
//!   any tantivy `Directory` (in memory, `component::search::ComponentSearch`)
//!   and keeps it up to date from the records each entry touched.
//! - [`records`]: a component app's table records as documents, shared by the node and
//!   the SDK's test harness; [`records::TableSearch`] is one table's index over one state.
//! - [`tokenize`]: Unicode words, accent/case folding, CJK bigrams, trigrams.

pub mod index;
pub mod records;
pub mod tokenize;
