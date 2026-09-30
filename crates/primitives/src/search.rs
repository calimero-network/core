//! The wire between an app and the node's full-text search.
//!
//! Two directions share these types:
//!
//! - **Extraction** (node → app): the node's indexer knows which entity ids of a
//!   context changed, but not what they mean. It calls the app's
//!   [`EXTRACT_EXPORT`] with an [`ExtractRequest`]; the app reads those ids
//!   from its own state and answers one `Option<SearchDoc>` per id
//!   ([`ExtractResponse`]). `None` means "not a document of this index (or no
//!   longer one)", which the indexer turns into a delete, so replaying an id is
//!   always safe. The app declares its indexes through [`SCHEMA_EXPORT`]
//!   ([`SearchIndexSchema`]), and pages through every document for a full
//!   build through [`SCAN_EXPORT`] ([`ScanRequest`]).
//! - **Query** (app → node): a view calls the `search_query` host function with
//!   a [`SearchRequest`] and gets a [`SearchResponse`]. There is deliberately no
//!   context field: the host binds the query to the context the view runs in.
//!
//! The SDK's `search_indexes!` macro generates the three exports; both sides
//! speak borsh, raw, in the execution's input and return value.
//!
//! All of it is node-local: nothing here is ever part of a delta or a snapshot.

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

/// An index an app declares, as its [`SCHEMA_EXPORT`] returns it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "borsh", derive(BorshDeserialize, BorshSerialize))]
pub struct SearchIndexSchema {
    /// The index's name, unique within the app.
    pub name: String,
    /// Bumped by the app whenever what it extracts changes meaning; a bump
    /// makes the node rebuild the index from state.
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

/// A document the app hands the indexer.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "borsh", derive(BorshDeserialize, BorshSerialize))]
pub struct SearchDoc {
    /// The entity id the document is derived from. It is the document's
    /// identity in the index: re-extracting an id replaces its document.
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
    /// Where to resume: all zeros for the first page, then the previous
    /// page's `next`. Opaque to the node, which only requires it to grow
    /// (compared as bytes); the SDK passes the lowest entity id the page may
    /// return, which stays put while entries come and go.
    pub from: [u8; 32],
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
    pub next: Option<[u8; 32]>,
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

/// Input of the `search_query` host function.
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
    /// Hits to skip (the `next_cursor` of a previous page).
    pub cursor: u32,
    /// Hits to return at most.
    pub limit: u32,
}

/// One hit.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "borsh", derive(BorshDeserialize, BorshSerialize))]
pub struct SearchHit {
    /// The entity id the document came from. The index may lag state, so the
    /// caller re-reads it and drops a hit whose entity no longer matches.
    pub id: [u8; 32],
    /// BM25 score (0 for a filter-only query).
    pub score: f32,
    /// A highlighted fragment of the best-matching text field, `<b>`-marked.
    pub snippet: String,
}

/// Output of the `search_query` host function.
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
