//! Node-local, never-synced, per-context full-text search (PoC).
//!
//! - [`directory`]: a tantivy `Directory` over the store's `SearchIndex`
//!   column, so an index is encrypted at rest with the rest of the node's data
//!   and dropped with one range delete.
//! - [`dirty`]: the `SearchDirty` log the apply path writes in the same batch
//!   as the state change, and the indexer drains.
//! - [`index`]: one context's index — schema from the app's declaration,
//!   idempotent delete-then-add writes, BM25 queries with filters.
//! - [`service`]: the indexer loop and the query entry point, which takes the
//!   context from its caller (the host), never from the request.
//! - [`tokenize`]: Unicode words, accent/case folding, CJK bigrams, trigrams.

pub mod directory;
pub mod dirty;
pub mod index;
pub mod service;
pub mod tokenize;

pub use service::{ContextKey, Extractor, IndexReport, SearchConfig, SearchService};
