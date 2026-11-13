//! The vocabulary between the node's full-text search and `calimero-search`.
//!
//! The node (`crates/node/src/component/search.rs`) builds one index per table
//! whose fields an app's ABI marks searchable (`state.search`): a
//! [`SearchIndexSchema`] of `Text` fields, one [`SearchDoc`] per stored record
//! (its id the SHA-256 of the record's stored key), and `calimero-search`'s
//! `ContextIndex` holds them in memory. A view's call to the WIT `search.query`
//! becomes a [`SearchRequest`] and comes back as a [`SearchResponse`]. There is
//! no app field: the node binds the query to the app the view runs in.
//!
//! The export names ([`SCHEMA_EXPORT`], [`EXTRACT_EXPORT`], [`SCAN_EXPORT`],
//! [`is_export`]) and their request and response types are the SDK 0.12 app
//! side, which no current code calls.
//!
//! All of it is node-local: derived from state, never in the log or a snapshot.

#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

/// Name of the app export that lists the app's search indexes. No input;
/// returns a borsh `Vec<SearchIndexSchema>`.
pub const SCHEMA_EXPORT: &str = "__calimero_search_schema";

/// Name of the app export that turns entity ids into documents: a borsh
/// [`ExtractRequest`] in, a borsh [`ExtractResponse`] out. Its presence in a
/// module is what opts the app into search.
pub const EXTRACT_EXPORT: &str = "__calimero_search_extract";

/// Name of the app export that pages through every document of an index, for
/// a full (re)build: a borsh [`ScanRequest`] in, a borsh [`ScanResponse`] out.
pub const SCAN_EXPORT: &str = "__calimero_search_scan";

/// Whether `method` is one of the search exports. They only ever read state,
/// whatever the app's ABI says, so the node runs them as views.
#[must_use]
pub fn is_export(method: &str) -> bool {
    matches!(method, SCHEMA_EXPORT | EXTRACT_EXPORT | SCAN_EXPORT)
}

/// How one field of a document is indexed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "borsh", derive(BorshDeserialize, BorshSerialize))]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SearchFieldKind {
    /// Tokenized free text, BM25-scored with `weight` as the field boost. With
    /// `infix`, a trigram twin field is indexed too, for substring match.
    Text {
        /// Boost of this field's BM25 score, in hundredths (`100` = 1.0).
        weight: u32,
        /// Also index trigrams, for `SearchMode::Substring`.
        infix: bool,
    },
    /// An exact keyword (a sender, a channel): filterable, never scored.
    Keyword,
    /// An unsigned integer (a timestamp): range-filterable and sortable.
    U64,
}

/// One field of a [`SearchIndexSchema`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "borsh", derive(BorshDeserialize, BorshSerialize))]
pub struct SearchFieldSchema {
    /// The field's name, as a [`SearchDoc`] names it.
    pub name: String,
    /// How it is indexed.
    pub kind: SearchFieldKind,
}

/// One index: the node makes one per table with searchable fields.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "borsh", derive(BorshDeserialize, BorshSerialize))]
pub struct SearchIndexSchema {
    /// The index's name, unique within the app (the node uses the table's).
    pub name: String,
    /// Bumped whenever what the documents mean changes; a bump makes the
    /// index rebuild from state (the node's in-memory indexes use `1`).
    pub version: u32,
    /// The fields of every document.
    pub fields: Vec<SearchFieldSchema>,
}

/// A field's value in a [`SearchDoc`].
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "borsh", derive(BorshDeserialize, BorshSerialize))]
#[serde(untagged)]
pub enum SearchValue {
    /// For a `Text` or a `Keyword` field.
    Str(String),
    /// For a `U64` field.
    U64(u64),
}

/// A document the node hands the indexer.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "borsh", derive(BorshDeserialize, BorshSerialize))]
pub struct SearchDoc {
    /// The document's identity in the index (the node uses the SHA-256 of the
    /// record's stored key): indexing an id again replaces its document.
    pub id: [u8; 32],
    /// `(field name, value)` pairs; a name the schema does not declare is
    /// ignored.
    pub fields: Vec<(String, SearchValue)>,
}

/// Input of [`EXTRACT_EXPORT`].
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "borsh", derive(BorshDeserialize, BorshSerialize))]
pub struct ExtractRequest {
    /// Which of the app's indexes the documents are for.
    pub index: String,
    /// The entity ids that changed.
    pub ids: Vec<[u8; 32]>,
}

/// Output of [`EXTRACT_EXPORT`]: one entry per requested id, in order.
pub type ExtractResponse = Vec<Option<SearchDoc>>;

/// Input of [`SCAN_EXPORT`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "borsh", derive(BorshDeserialize, BorshSerialize))]
pub struct ScanRequest {
    /// Which of the app's indexes to page through.
    pub index: String,
    /// Where to resume: empty for the first page, then the previous page's
    /// `next`. Opaque to the node, which only requires it to grow (compared as
    /// bytes). The SDK writes which of the index's collections it is paging
    /// and the lowest entity id the page may return, which stays put while
    /// entries come and go.
    pub from: Vec<u8>,
    /// Documents to return at most.
    pub limit: u32,
}

/// Output of [`SCAN_EXPORT`].
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "borsh", derive(BorshDeserialize, BorshSerialize))]
pub struct ScanResponse {
    /// This page's documents.
    pub docs: Vec<SearchDoc>,
    /// The `from` of the next page, `None` on the last one.
    pub next: Option<Vec<u8>>,
}

/// How the query string is matched.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "borsh", derive(BorshDeserialize, BorshSerialize))]
#[serde(rename_all = "camelCase")]
pub enum SearchMode {
    /// Every word must occur (AND), ranked by BM25 over the text fields.
    #[default]
    Words,
    /// As `Words`, with the last word matched as a prefix (search-as-you-type).
    Prefix,
    /// The query is a substring of a text field (case/accent-insensitive),
    /// answered from the trigram field. Needs 3+ characters.
    Substring,
    /// As `Words`, each word within edit distance 1.
    Fuzzy,
}

/// The order hits come back in.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "borsh", derive(BorshDeserialize, BorshSerialize))]
#[serde(tag = "by", rename_all = "camelCase")]
pub enum SearchOrder {
    /// Best match first (BM25). A filter-only query scores every hit 0.
    #[default]
    Relevance,
    /// By a `U64` field: newest first, say, for a timestamp. A hit without the
    /// field sorts after every hit with it.
    Field {
        /// The field.
        field: String,
        /// Largest first.
        descending: bool,
    },
}

/// A filter every hit must pass.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "borsh", derive(BorshDeserialize, BorshSerialize))]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum SearchFilter {
    /// A `Keyword` field equals the value.
    Eq {
        /// The field.
        field: String,
        /// The exact value.
        value: String,
    },
    /// A `U64` field lies in `[min, max]`.
    Range {
        /// The field.
        field: String,
        /// Inclusive lower bound.
        min: u64,
        /// Inclusive upper bound.
        max: u64,
    },
}

/// A query: what the node makes of a view's WIT `search.query` call.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "borsh", derive(BorshDeserialize, BorshSerialize))]
pub struct SearchRequest {
    /// Which index to query.
    pub index: String,
    /// The user's query text.
    pub query: String,
    /// How to match it.
    pub mode: SearchMode,
    /// Filters every hit must pass.
    pub filters: Vec<SearchFilter>,
    /// The order hits come back in.
    pub order: SearchOrder,
    /// Hits to skip (the `next_cursor` of a previous page).
    pub cursor: u32,
    /// Hits to return at most.
    pub limit: u32,
}

/// One hit.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "borsh", derive(BorshDeserialize, BorshSerialize))]
pub struct SearchHit {
    /// The id of the document that matched. The node maps it back to the
    /// record's key and drops a hit whose record is no longer indexed.
    pub id: [u8; 32],
    /// BM25 score (0 for a filter-only query).
    pub score: f32,
    /// A highlighted fragment of the best-matching text field, `<b>`-marked.
    pub snippet: String,
}

/// The answer to a [`SearchRequest`].
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "borsh", derive(BorshDeserialize, BorshSerialize))]
pub struct SearchResponse {
    /// The page of hits, best first.
    pub hits: Vec<SearchHit>,
    /// The cursor of the next page, `None` on the last one.
    pub next_cursor: Option<u32>,
    /// Documents the query matched in all.
    pub total: u64,
}
