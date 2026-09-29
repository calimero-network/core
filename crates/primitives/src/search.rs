//! The wire between an app and the node's full-text search (PoC).
//!
//! Two directions share these types:
//!
//! - **Extraction** (node → app): the node's indexer knows which entity ids of a
//!   context changed, but not what they mean. It calls the app's
//!   `search_poc_extract` method with an [`ExtractRequest`]; the app
//!   reads those ids from its own state and answers one `Option<SearchDoc>` per
//!   id ([`ExtractResponse`]). `None` means "not a document of this index (or no
//!   longer one)", which the indexer turns into a delete, so replaying an id is
//!   always safe. The app declares its indexes once through
//!   `search_poc_schema` ([`SearchIndexSchema`]), and pages through
//!   every document for a full build through `search_poc_scan`
//!   ([`ScanRequest`]).
//! - **Query** (app → node): a view calls the `search_query` host function with
//!   a [`SearchRequest`] and gets a [`SearchResponse`]. There is deliberately no
//!   context field: the host binds the query to the context the view runs in.
//!
//! All of it is node-local: nothing here is ever part of a delta or a snapshot.

#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

// The PoC's apps write these three as ordinary `#[app::view]` methods, which
// is why they are not `__calimero_*`: the SDK reserves that prefix for its own
// code generation, and a production `#[app::search_index]` macro would emit
// them under it (see the PoC results doc). Being views, they run read-only
// against current state under the context's shared lock, JSON in and out.

/// Name of the app method that lists the app's search indexes.
pub const SCHEMA_EXPORT: &str = "search_poc_schema";

/// Name of the app method that turns entity ids into documents.
pub const EXTRACT_EXPORT: &str = "search_poc_extract";

/// Name of the app method that pages through every document of an index, for
/// a full (re)build.
pub const SCAN_EXPORT: &str = "search_poc_scan";

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

/// An index an app declares. The macro-generated form of this would live in
/// the app's embedded ABI; the PoC reads it from `search_poc_schema`.
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

/// Input of `search_poc_extract`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "borsh", derive(BorshDeserialize, BorshSerialize))]
pub struct ExtractRequest {
    /// Which of the app's indexes the documents are for.
    pub index: String,
    /// The entity ids that changed.
    pub ids: Vec<[u8; 32]>,
}

/// Output of `search_poc_extract`: one entry per requested id, in order.
pub type ExtractResponse = Vec<Option<SearchDoc>>;

/// Input of `search_poc_scan`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "borsh", derive(BorshDeserialize, BorshSerialize))]
pub struct ScanRequest {
    /// Which of the app's indexes to page through.
    pub index: String,
    /// Where to resume: `0` for the first page, then the previous page's
    /// `next`. Opaque to the node, which only requires it to grow; the app
    /// picks what it counts (search-chat: child-trie buckets, which stay put
    /// while entries come and go).
    pub offset: u32,
    /// Documents to return at most.
    pub limit: u32,
}

/// Output of `search_poc_scan`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "borsh", derive(BorshDeserialize, BorshSerialize))]
pub struct ScanResponse {
    /// This page's documents.
    pub docs: Vec<SearchDoc>,
    /// The `offset` of the next page, `None` on the last one.
    pub next: Option<u32>,
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
