//! Full-text search over the node's index of this context.
//!
//! An app opts in with two declarations and no hand-written plumbing:
//!
//! 1. `#[derive(app::Searchable)]` on a collection's value type says which of
//!    its fields are indexed, and how (`#[search(text)]`, `#[search(keyword)]`,
//!    `#[search(number)]`).
//! 2. [`search_indexes!`](crate::search_indexes) names the state's collections
//!    that are indexes, generating the exports the node's indexer calls
//!    (`__calimero_search_schema`, `__calimero_search_extract`,
//!    `__calimero_search_scan`).
//!
//! A view then queries through the collection, typed: a [`Query`] in,
//! [`Results`] of the collection's own keys and values out, every hit re-read
//! from state.
//!
//! ```ignore
//! #[derive(BorshSerialize, BorshDeserialize, app::Searchable)]
//! pub struct Message {
//!     #[search(keyword)]
//!     pub sender: LwwRegister<String>,
//!     #[search(text, infix)]
//!     pub text: LwwRegister<String>,
//!     #[search(number)]
//!     pub ts: LwwRegister<u64>,
//! }
//!
//! app::search_indexes!(Chat {
//!     "messages" (version = 1) => messages,
//! });
//!
//! #[app::view]
//! pub fn search(&self, q: String) -> app::Result<Vec<String>> {
//!     let results = self.messages.search("messages", &Query::words(q).limit(20))?;
//!     Ok(results.hits.into_iter().map(|hit| hit.value.text.get().clone()).collect())
//! }
//! ```
//!
//! The index lives on each node, never in a delta: it lags state by up to the
//! indexer's commit interval, and only views may query it (a write that
//! searched would record a decision other nodes cannot reproduce). The node
//! charges a query gas for the work it does; see the docs site's search page.

use core::fmt;

use crate::env;
pub use calimero_primitives::search::{
    ExtractRequest, ExtractResponse, ScanRequest, ScanResponse, SearchDoc, SearchFieldKind,
    SearchFieldSchema, SearchFilter, SearchHit, SearchIndexSchema, SearchMode, SearchOrder,
    SearchRequest, SearchResponse, SearchValue,
};

/// Why a search did not answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchError {
    /// The node refused the request (an unknown field, a substring shorter
    /// than 3 characters, a malformed request), with its reason.
    Refused(String),
    /// Reading a hit (or a document) back from state failed.
    Storage(String),
}

impl fmt::Display for SearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(reason) => write!(f, "search refused: {reason}"),
            Self::Storage(reason) => write!(f, "search could not read state: {reason}"),
        }
    }
}

impl core::error::Error for SearchError {}

/// A value that indexes as text: a `#[search(text)]` or `#[search(keyword)]`
/// field. `None` leaves the field out of the document.
pub trait SearchText {
    /// The text to index.
    fn search_text(&self) -> Option<String>;
}

/// A value that indexes as an unsigned integer: a `#[search(number)]` field,
/// range-filterable. `None` leaves the field out of the document.
pub trait SearchNumber {
    /// The number to index.
    fn search_number(&self) -> Option<u64>;
}

impl SearchText for String {
    fn search_text(&self) -> Option<String> {
        Some(self.clone())
    }
}

impl SearchText for str {
    fn search_text(&self) -> Option<String> {
        Some(self.to_owned())
    }
}

impl<T: SearchText + ?Sized> SearchText for &T {
    fn search_text(&self) -> Option<String> {
        (**self).search_text()
    }
}

impl<T: SearchText> SearchText for Option<T> {
    fn search_text(&self) -> Option<String> {
        self.as_ref().and_then(SearchText::search_text)
    }
}

macro_rules! numbers {
    ($($ty:ty),*) => {$(
        impl SearchNumber for $ty {
            fn search_number(&self) -> Option<u64> {
                Some(u64::from(*self))
            }
        }
    )*};
}

numbers!(u8, u16, u32, u64);

impl<T: SearchNumber + ?Sized> SearchNumber for &T {
    fn search_number(&self) -> Option<u64> {
        (**self).search_number()
    }
}

impl<T: SearchNumber> SearchNumber for Option<T> {
    fn search_number(&self) -> Option<u64> {
        self.as_ref().and_then(SearchNumber::search_number)
    }
}

/// A value whose fields are indexed. Derive it with `#[derive(app::Searchable)]`.
pub trait Searchable {
    /// The fields every document of this type has, as the index declares
    /// them.
    fn search_fields() -> Vec<SearchFieldSchema>;

    /// This value's fields, as the indexer stores them.
    fn search_document(&self) -> Vec<(String, SearchValue)>;

    /// Whether this value is in the index at all. `#[search(index_if = f)]`
    /// sets it: a soft-deleted message, say, is not. Every value is by
    /// default.
    fn search_indexed(&self) -> bool {
        true
    }
}

/// A collection's entry: its key and value.
pub type Entry<C> = (<C as SearchCollection>::Key, <C as SearchCollection>::Value);

/// One page of entry ids from [`SearchCollection::search_page`], and the bound
/// to resume from (`None` after the last page).
pub type Page = (Vec<[u8; 32]>, Option<[u8; 32]>);

/// A collection an index can be built over: its entries are found by entity
/// id (what the node hands back) and paged through in a stable order.
///
/// `calimero-storage` implements it for `UnorderedMap<K, V>` with
/// `V: Searchable`.
pub trait SearchCollection {
    /// The collection's key.
    type Key;
    /// The collection's value, whose fields are indexed.
    type Value: Searchable;

    /// The entry stored under entity `id`, if `id` is a live entry of *this*
    /// collection. Anything else — another collection's entry, a deleted one,
    /// a forged id — is `None`.
    ///
    /// # Errors
    /// A storage failure.
    fn search_entry(&self, id: [u8; 32]) -> Result<Option<Entry<Self>>, SearchError>;

    /// A page of entry ids at or above `from` in byte order, at least
    /// `at_least` of them unless the collection runs out, and the bound to
    /// resume from (`None` after the last page), above every id the page
    /// returned. A bound on ids stays put while entries come and go, so a page
    /// never skips an entry that existed throughout.
    ///
    /// # Errors
    /// A storage failure.
    fn search_page(&self, from: [u8; 32], at_least: usize) -> Result<Page, SearchError>;

    /// One document or `None` per id, for the indexer.
    ///
    /// # Errors
    /// A storage failure.
    fn search_extract(&self, ids: &[[u8; 32]]) -> Result<ExtractResponse, SearchError> {
        ids.iter()
            .map(|id| {
                Ok(self
                    .search_entry(*id)?
                    .filter(|(_, value)| value.search_indexed())
                    .map(|(_, value)| SearchDoc {
                        id: *id,
                        fields: value.search_document(),
                    }))
            })
            .collect()
    }

    /// One page of every document, for a full build: the page's documents
    /// and, in `next`, the 32-byte id bound to resume from.
    ///
    /// # Errors
    /// A storage failure.
    fn search_scan(&self, from: [u8; 32], limit: u32) -> Result<ScanResponse, SearchError> {
        let (ids, next) = self.search_page(from, limit as usize)?;
        let docs = self.search_extract(&ids)?.into_iter().flatten().collect();
        Ok(ScanResponse {
            docs,
            next: next.map(Vec::from),
        })
    }

    /// Search index `index` of this context, and read every hit back from
    /// this collection. A hit whose entry is gone, or no longer indexed (the
    /// index lags state), is dropped and counted in [`Results::stale`]. For an
    /// index over several collections, read hits back from each with
    /// [`search_entry`](Self::search_entry) instead (see
    /// [`search_indexes!`](crate::search_indexes)).
    ///
    /// Only a view may call it: in a mutating method the node traps the run.
    ///
    /// # Errors
    /// The node's refusal, or a storage failure reading a hit.
    fn search(
        &self,
        index: &str,
        query: &Query,
    ) -> Result<Results<Self::Key, Self::Value>, SearchError> {
        let response = query.run(index)?;
        let mut hits = Vec::with_capacity(response.hits.len());
        let mut stale = 0;
        for hit in response.hits {
            match self
                .search_entry(hit.id)?
                .filter(|(_, v)| v.search_indexed())
            {
                Some((key, value)) => hits.push(Hit {
                    key,
                    value,
                    score: hit.score,
                    snippet: hit.snippet,
                }),
                None => stale += 1,
            }
        }
        Ok(Results {
            hits,
            total: response.total,
            next_cursor: response.next_cursor,
            stale,
        })
    }
}

/// A search, built up from the text and how to match it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    text: String,
    mode: SearchMode,
    filters: Vec<SearchFilter>,
    order: SearchOrder,
    cursor: u32,
    limit: u32,
}

impl Query {
    /// Hits per page unless [`limit`](Self::limit) says otherwise.
    pub const DEFAULT_LIMIT: u32 = 20;

    fn new(text: impl Into<String>, mode: SearchMode) -> Self {
        Self {
            text: text.into(),
            mode,
            filters: Vec::new(),
            order: SearchOrder::Relevance,
            cursor: 0,
            limit: Self::DEFAULT_LIMIT,
        }
    }

    /// Every word must occur, ranked by BM25.
    pub fn words(text: impl Into<String>) -> Self {
        Self::new(text, SearchMode::Words)
    }

    /// As [`words`](Self::words), the last word matched as a prefix
    /// (search-as-you-type).
    pub fn prefix(text: impl Into<String>) -> Self {
        Self::new(text, SearchMode::Prefix)
    }

    /// The text occurs inside a `#[search(text, infix)]` field, case- and
    /// accent-insensitive. Needs 3 or more characters.
    pub fn substring(text: impl Into<String>) -> Self {
        Self::new(text, SearchMode::Substring)
    }

    /// As [`words`](Self::words), each word within one edit.
    pub fn fuzzy(text: impl Into<String>) -> Self {
        Self::new(text, SearchMode::Fuzzy)
    }

    /// Every document, unranked: pair it with filters.
    #[must_use]
    pub fn all() -> Self {
        Self::new(String::new(), SearchMode::Words)
    }

    /// Only hits whose keyword `field` is exactly `value`.
    #[must_use]
    pub fn eq(mut self, field: impl Into<String>, value: impl Into<String>) -> Self {
        self.filters.push(SearchFilter::Eq {
            field: field.into(),
            value: value.into(),
        });
        self
    }

    /// Only hits whose number `field` lies in `min..=max`.
    #[must_use]
    pub fn range(mut self, field: impl Into<String>, min: u64, max: u64) -> Self {
        self.filters.push(SearchFilter::Range {
            field: field.into(),
            min,
            max,
        });
        self
    }

    /// Largest `field` first, instead of best match first: newest first, for a
    /// timestamp. `field` must be a `#[search(number)]` field.
    #[must_use]
    pub fn newest_first(mut self, field: impl Into<String>) -> Self {
        self.order = SearchOrder::Field {
            field: field.into(),
            descending: true,
        };
        self
    }

    /// Smallest `field` first. `field` must be a `#[search(number)]` field.
    #[must_use]
    pub fn oldest_first(mut self, field: impl Into<String>) -> Self {
        self.order = SearchOrder::Field {
            field: field.into(),
            descending: false,
        };
        self
    }

    /// Start after `cursor` hits (a previous page's
    /// [`next_cursor`](Results::next_cursor)).
    #[must_use]
    pub const fn cursor(mut self, cursor: u32) -> Self {
        self.cursor = cursor;
        self
    }

    /// At most `limit` hits (the node caps a page at 100).
    #[must_use]
    pub const fn limit(mut self, limit: u32) -> Self {
        self.limit = limit;
        self
    }

    /// The request this query sends for `index`.
    #[must_use]
    pub fn request(&self, index: &str) -> SearchRequest {
        SearchRequest {
            index: index.to_owned(),
            query: self.text.clone(),
            mode: self.mode,
            filters: self.filters.clone(),
            order: self.order.clone(),
            cursor: self.cursor,
            limit: self.limit,
        }
    }

    /// Ask the node, and get its raw answer: hits as entity ids. Most views
    /// want [`SearchCollection::search`], which reads them back.
    ///
    /// # Errors
    /// The node's refusal.
    pub fn run(&self, index: &str) -> Result<SearchResponse, SearchError> {
        env::search(&self.request(index)).map_err(SearchError::Refused)
    }
}

/// One hit, read back from state.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit<K, V> {
    /// The entry's key.
    pub key: K,
    /// The entry's current value.
    pub value: V,
    /// BM25 score (0 for a filter-only query).
    pub score: f32,
    /// A highlighted fragment of the best-matching text field, `<b>`-marked.
    pub snippet: String,
}

/// A page of hits. A view maps it onto its own return type (which carries
/// the app's ABI), typically through [`Results::map`].
#[derive(Clone, Debug, PartialEq)]
pub struct Results<K, V> {
    /// This page's hits, best first.
    pub hits: Vec<Hit<K, V>>,
    /// Documents the index matched in all (before stale hits are dropped).
    pub total: u64,
    /// Pass to [`Query::cursor`] for the next page; `None` on the last.
    pub next_cursor: Option<u32>,
    /// Hits the index returned whose entry is gone from state.
    pub stale: u32,
}

impl<K, V> Results<K, V> {
    /// The same page with every hit's value mapped (to a view type, say).
    pub fn map<W>(self, mut f: impl FnMut(&K, V) -> W) -> Results<K, W> {
        Results {
            hits: self
                .hits
                .into_iter()
                .map(|hit| Hit {
                    value: f(&hit.key, hit.value),
                    key: hit.key,
                    score: hit.score,
                    snippet: hit.snippet,
                })
                .collect(),
            total: self.total,
            next_cursor: self.next_cursor,
            stale: self.stale,
        }
    }
}

/// What [`search_indexes!`](crate::search_indexes) expands to call.
#[doc(hidden)]
pub mod __private {
    use borsh::{BorshDeserialize, BorshSerialize};

    use super::{
        ExtractResponse, ScanResponse, SearchCollection, SearchError, SearchIndexSchema, Searchable,
    };
    use crate::env;

    /// One collection of an index, as the exports read it: the part of a
    /// [`SearchCollection`] that does not depend on its key type, so the
    /// collections of one index can sit side by side.
    pub trait Part {
        /// [`SearchCollection::search_extract`].
        fn extract(&self, ids: &[[u8; 32]]) -> Result<ExtractResponse, SearchError>;
        /// [`SearchCollection::search_scan`].
        fn scan(&self, from: [u8; 32], limit: u32) -> Result<ScanResponse, SearchError>;
    }

    impl<C: SearchCollection> Part for C {
        fn extract(&self, ids: &[[u8; 32]]) -> Result<ExtractResponse, SearchError> {
            self.search_extract(ids)
        }

        fn scan(&self, from: [u8; 32], limit: u32) -> Result<ScanResponse, SearchError> {
            self.search_scan(from, limit)
        }
    }

    /// The schema of an index named `name` over `collection`.
    pub fn schema<C: SearchCollection>(_: &C, name: &str, version: u32) -> SearchIndexSchema {
        SearchIndexSchema {
            name: name.to_owned(),
            version,
            fields: <C::Value as Searchable>::search_fields(),
        }
    }

    /// Every collection of one index holds the same value type, so the index
    /// has one schema. A compile error otherwise.
    pub const fn same_value<A, B>(_: &A, _: &B)
    where
        A: SearchCollection,
        B: SearchCollection<Value = A::Value>,
    {
    }

    /// Each id's document, from the first of `parts` it is an entry of.
    ///
    /// # Errors
    /// A storage failure.
    pub fn extract(parts: &[&dyn Part], ids: &[[u8; 32]]) -> Result<ExtractResponse, SearchError> {
        let mut out: ExtractResponse = ids.iter().map(|_| None).collect();
        for part in parts {
            let pending: Vec<usize> = (0..ids.len()).filter(|i| out[*i].is_none()).collect();
            if pending.is_empty() {
                break;
            }
            let asked: Vec<[u8; 32]> = pending.iter().map(|i| ids[*i]).collect();
            for (i, doc) in pending.into_iter().zip(part.extract(&asked)?) {
                out[i] = doc;
            }
        }
        Ok(out)
    }

    /// One page of the index's documents. The cursor is the collection being
    /// paged (one byte) and the id bound within it, so it grows across the
    /// whole index: through one collection, then on to the next.
    ///
    /// # Errors
    /// A malformed cursor, or a storage failure.
    pub fn scan(parts: &[&dyn Part], from: &[u8], limit: u32) -> Result<ScanResponse, SearchError> {
        let (at, bound) = match from {
            [] => (0, [0; 32]),
            [at, bound @ ..] => (usize::from(*at), id_bound(bound)?),
        };
        let Some(part) = parts.get(at) else {
            return Ok(ScanResponse::default());
        };
        let page = part.scan(bound, limit)?;
        let next = match page.next {
            Some(bound) => Some(cursor(at, id_bound(&bound)?)),
            None => (at + 1 < parts.len()).then(|| cursor(at + 1, [0; 32])),
        };
        Ok(ScanResponse {
            docs: page.docs,
            next,
        })
    }

    fn id_bound(bytes: &[u8]) -> Result<[u8; 32], SearchError> {
        <[u8; 32]>::try_from(bytes)
            .map_err(|_| SearchError::Refused("a malformed scan cursor".to_owned()))
    }

    fn cursor(at: usize, bound: [u8; 32]) -> Vec<u8> {
        // SAFETY of the cast: the macro takes a handful of collections per
        // index, far under 256; a larger `at` would only end the scan early.
        let mut out = vec![u8::try_from(at).unwrap_or(u8::MAX)];
        out.extend_from_slice(&bound);
        out
    }

    /// Decode the export's borsh input.
    pub fn input<T: BorshDeserialize>() -> Result<T, SearchError> {
        let bytes = env::input().unwrap_or_default();
        borsh::from_slice(&bytes).map_err(|e| SearchError::Refused(e.to_string()))
    }

    /// Return `out` borsh-encoded, or the error as the call's failure.
    pub fn respond<T: BorshSerialize>(out: Result<T, SearchError>) {
        let out = out.and_then(|value| {
            borsh::to_vec(&value).map_err(|e| SearchError::Storage(e.to_string()))
        });
        match out {
            Ok(bytes) => env::value_return::<_, Vec<u8>>(&Ok(bytes)),
            Err(err) => env::value_return::<Vec<u8>, _>(&Err(err.to_string().into_bytes())),
        }
    }
}

/// Declare which collections of the app's state are search indexes.
///
/// ```ignore
/// app::search_indexes!(Chat {
///     "messages" (version = 1) => messages | replies,
///     "channels" (version = 1) => channels,
/// });
/// ```
///
/// Each entry names an index, its version, and the state fields it indexes;
/// each field must be a [`SearchCollection`] whose value is [`Searchable`].
/// Fields joined by `|` make one index over several collections of the same
/// value type (top-level messages and thread replies, say): one ranking and
/// one cursor across them. A view reads such an index's hits back with
/// [`Query::run`] and each collection's
/// [`search_entry`](SearchCollection::search_entry), the first that holds the
/// id. Bump `version` whenever what a document holds changes meaning without
/// the declared fields changing: the node then rebuilds the index from state.
/// (A change to the fields rebuilds it anyway.)
///
/// The macro generates the three exports the node's indexer calls. It is the
/// app's whole opt-in: an app that does not use it exports none of them, and
/// the node writes nothing and runs nothing for it.
#[macro_export]
macro_rules! search_indexes {
    ($state:ty { $( $name:literal (version = $version:expr) => $field:ident $(| $more:ident)* ),+ $(,)? }) => {
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn __calimero_search_schema() {
            $crate::env::setup_panic_hook();
            let Some(app) = ::calimero_storage::collections::Root::<$state>::fetch() else {
                $crate::env::panic_str("Failed to find or read app state")
            };
            $crate::search::__private::respond(::core::result::Result::Ok(
                ::std::vec![$({
                    $( $crate::search::__private::same_value(&app.$field, &app.$more); )*
                    $crate::search::__private::schema(&app.$field, $name, $version)
                }),+],
            ));
        }

        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn __calimero_search_extract() {
            $crate::env::setup_panic_hook();
            let Some(app) = ::calimero_storage::collections::Root::<$state>::fetch() else {
                $crate::env::panic_str("Failed to find or read app state")
            };
            $crate::search::__private::respond(
                $crate::search::__private::input::<$crate::search::ExtractRequest>().and_then(
                    |request| match request.index.as_str() {
                        $( $name => $crate::search::__private::extract(
                            &[&app.$field as &dyn $crate::search::__private::Part $(, &app.$more)*],
                            &request.ids,
                        ), )+
                        _ => ::core::result::Result::Ok(
                            request.ids.iter().map(|_| ::core::option::Option::None).collect(),
                        ),
                    },
                ),
            );
        }

        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn __calimero_search_scan() {
            $crate::env::setup_panic_hook();
            let Some(app) = ::calimero_storage::collections::Root::<$state>::fetch() else {
                $crate::env::panic_str("Failed to find or read app state")
            };
            $crate::search::__private::respond(
                $crate::search::__private::input::<$crate::search::ScanRequest>().and_then(
                    |request| match request.index.as_str() {
                        $( $name => $crate::search::__private::scan(
                            &[&app.$field as &dyn $crate::search::__private::Part $(, &app.$more)*],
                            &request.from,
                            request.limit,
                        ), )+
                        _ => ::core::result::Result::Ok($crate::search::ScanResponse::default()),
                    },
                ),
            );
        }
    };
}

#[cfg(test)]
mod tests {
    use super::__private::{extract, scan, Part};
    use super::*;

    /// A value with one text field.
    #[derive(Clone, Debug, PartialEq)]
    struct Doc(&'static str, bool);

    impl Searchable for Doc {
        fn search_fields() -> Vec<SearchFieldSchema> {
            Vec::new()
        }

        fn search_document(&self) -> Vec<(String, SearchValue)> {
            vec![("text".to_owned(), SearchValue::Str(self.0.to_owned()))]
        }

        fn search_indexed(&self) -> bool {
            self.1
        }
    }

    /// A collection of `(id, doc)`, sorted by id.
    struct Fake(Vec<([u8; 32], Doc)>);

    impl SearchCollection for Fake {
        type Key = [u8; 32];
        type Value = Doc;

        fn search_entry(&self, id: [u8; 32]) -> Result<Option<Entry<Self>>, SearchError> {
            Ok(self.0.iter().find(|(i, _)| *i == id).cloned())
        }

        fn search_page(&self, from: [u8; 32], at_least: usize) -> Result<Page, SearchError> {
            let mut rest = self
                .0
                .iter()
                .filter(|(id, _)| *id >= from)
                .map(|(id, _)| *id);
            let page: Vec<[u8; 32]> = rest.by_ref().take(at_least).collect();
            Ok((page, rest.next()))
        }
    }

    fn id(n: u8) -> [u8; 32] {
        [n; 32]
    }

    fn fake(ids: &[u8]) -> Fake {
        Fake(ids.iter().map(|n| (id(*n), Doc("x", true))).collect())
    }

    #[test]
    fn a_scan_walks_every_collection_of_an_index_with_a_growing_cursor() {
        let (a, empty, b) = (fake(&[1, 2, 3]), fake(&[]), fake(&[4, 5]));
        let parts: [&dyn Part; 3] = [&a, &empty, &b];
        let mut from = Vec::new();
        let mut seen = Vec::new();
        loop {
            let page = scan(&parts, &from, 2).expect("scan");
            seen.extend(page.docs.iter().map(|d| d.id[0]));
            let Some(next) = page.next else { break };
            assert!(next > from, "the cursor grows: {from:?} -> {next:?}");
            from = next;
        }
        assert_eq!(seen, vec![1, 2, 3, 4, 5]);
        assert!(scan(&parts, &[0, 1, 2], 2).is_err(), "a truncated cursor");
    }

    #[test]
    fn an_id_is_extracted_from_the_first_collection_that_holds_it() {
        let a = Fake(vec![
            (id(1), Doc("in a", true)),
            (id(2), Doc("hidden", false)),
        ]);
        let b = Fake(vec![(id(1), Doc("in b", true)), (id(3), Doc("in b", true))]);
        let parts: [&dyn Part; 2] = [&a, &b];
        let docs = extract(&parts, &[id(1), id(2), id(3), id(9)]).expect("extract");
        let text = |i: usize| {
            docs[i].as_ref().map(|d| match &d.fields[0].1 {
                SearchValue::Str(s) => s.clone(),
                SearchValue::U64(_) => unreachable!(),
            })
        };
        assert_eq!(text(0).as_deref(), Some("in a"), "the first holder wins");
        assert_eq!(text(1), None, "a value that is not indexed has no document");
        assert_eq!(text(2).as_deref(), Some("in b"));
        assert_eq!(text(3), None, "an id no collection holds");
    }

    #[test]
    fn a_query_orders_by_a_number_field_when_asked() {
        assert_eq!(Query::words("x").request("i").order, SearchOrder::Relevance);
        assert_eq!(
            Query::words("x").newest_first("ts").request("i").order,
            SearchOrder::Field {
                field: "ts".to_owned(),
                descending: true
            }
        );
    }
}
