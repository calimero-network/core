//! Full-text search (spec section 7, R2): the host keeps an index of each table's declared
//! text fields from the app's state, and views query it. The index is derived data on the
//! node; the host core only carries the query to it and enforces who may ask.

/// Hits one query returns at most (the node's index caps it the same way).
pub const MAX_SEARCH_LIMIT: u32 = 100;
/// Bytes of a query's text.
pub const MAX_SEARCH_QUERY_BYTES: usize = 256;

/// How the query text is matched; mirrors the WIT `search-mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchMode {
    Words,
    Prefix,
    Substring,
    Fuzzy,
}

/// One query of a table's index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchQuery {
    /// The table (its state field name, camelCase).
    pub table: String,
    pub text: String,
    pub mode: SearchMode,
    /// Hits to skip: the `next` of the previous page.
    pub cursor: u32,
    pub limit: u32,
}

/// One hit: the record's key as the guest stores it (no generation prefix).
#[derive(Clone, Debug, PartialEq)]
pub struct SearchHit {
    pub key: Vec<u8>,
    pub score: f32,
    pub snippet: String,
}

/// A page of hits, best first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchPage {
    pub hits: Vec<SearchHit>,
    pub next: Option<u32>,
    pub total: u64,
}

/// The node's index of the app, brought up to the state the view reads.
pub trait SearchHost: Send + Sync {
    /// The page, or why there is none (no such table, no declared fields, a bad query):
    /// the WIT `result` error.
    fn query(&self, query: &SearchQuery) -> Result<SearchPage, String>;
}

/// A run without the node's index: every table answers that it is not searchable.
pub struct NoSearch;

impl SearchHost for NoSearch {
    fn query(&self, query: &SearchQuery) -> Result<SearchPage, String> {
        Err(format!("table {:?} declares no search fields", query.table))
    }
}
