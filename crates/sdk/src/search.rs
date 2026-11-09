//! Full-text search over a table's declared text fields (spec section 7, R2).
//!
//! A record declares the fields with `#[table(key = "id", search = ["title", "body"])]`;
//! the state ABI lists them, and the host keeps the index from the app's state, outside it.
//! A view queries it with [`crate::Table::search`]; calls and timers cannot (`HostDenied`).

/// How the query text is matched.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SearchMode {
    /// Every word must occur, ranked by relevance.
    #[default]
    Words,
    /// As `Words`, the last word matched as a prefix (search as you type).
    Prefix,
    /// The text is a substring of a field (3 characters or more).
    Substring,
    /// As `Words`, each word within one edit.
    Fuzzy,
}

/// Options of [`crate::Table::search`].
#[derive(Clone, Debug, Default)]
pub struct SearchOpts {
    /// How to match.
    pub mode: SearchMode,
    /// The `next` of the previous page.
    pub cursor: Option<u32>,
    /// Hits per page: 1 to 100; 20 when unset.
    pub limit: Option<u32>,
}

impl SearchOpts {
    /// Match with `mode`.
    #[must_use]
    pub fn mode(mut self, mode: SearchMode) -> Self {
        self.mode = mode;
        self
    }

    /// Continue from a page's `next`.
    #[must_use]
    pub fn cursor(mut self, cursor: u32) -> Self {
        self.cursor = Some(cursor);
        self
    }

    /// Set the page size.
    #[must_use]
    pub fn limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }
}

/// The page size of a search without a limit.
pub const DEFAULT_SEARCH_LIMIT: u32 = 20;

/// One record found, best first.
#[derive(Clone, Debug, PartialEq)]
pub struct Found<T> {
    /// The record as it is now.
    pub record: T,
    /// Relevance (BM25).
    pub score: f32,
    /// A fragment of the field that matched, matches marked `<b>..</b>`.
    pub snippet: String,
}

/// A page of [`Found`] records.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchResults<T> {
    /// This page.
    pub items: Vec<Found<T>>,
    /// The cursor of the next page.
    pub next: Option<u32>,
    /// How many records matched in all.
    pub total: u64,
}

/// One hit as the host answers it: the record's storage key.
#[derive(Clone, Debug, PartialEq)]
pub struct RawHit {
    /// The record's key, as the table stores it.
    pub key: Vec<u8>,
    /// Relevance.
    pub score: f32,
    /// The marked fragment.
    pub snippet: String,
}

/// A page as the host answers it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawPage {
    /// The hits.
    pub hits: Vec<RawHit>,
    /// The next cursor.
    pub next: Option<u32>,
    /// Matches in all.
    pub total: u64,
}
