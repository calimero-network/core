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
    SearchFieldSchema, SearchFilter, SearchHit, SearchIndexSchema, SearchMode, SearchRequest,
    SearchResponse, SearchValue,
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
}

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
    fn search_entry(&self, id: [u8; 32]) -> Result<Option<(Self::Key, Self::Value)>, SearchError>;

    /// A page of entry ids from position `from` on, at least `at_least` of
    /// them unless the collection runs out, and the position to resume from
    /// (`None` after the last page). Positions must stay put while entries
    /// come and go, so a page never skips an entry that existed throughout.
    ///
    /// # Errors
    /// A storage failure.
    fn search_page(
        &self,
        from: u32,
        at_least: usize,
    ) -> Result<(Vec<[u8; 32]>, Option<u32>), SearchError>;

    /// One document or `None` per id, for the indexer.
    ///
    /// # Errors
    /// A storage failure.
    fn search_extract(&self, ids: &[[u8; 32]]) -> Result<ExtractResponse, SearchError> {
        ids.iter()
            .map(|id| {
                Ok(self.search_entry(*id)?.map(|(_, value)| SearchDoc {
                    id: *id,
                    fields: value.search_document(),
                }))
            })
            .collect()
    }

    /// One page of every document, for a full build.
    ///
    /// # Errors
    /// A storage failure.
    fn search_scan(&self, offset: u32, limit: u32) -> Result<ScanResponse, SearchError> {
        let (ids, next) = self.search_page(offset, limit as usize)?;
        let docs = self.search_extract(&ids)?.into_iter().flatten().collect();
        Ok(ScanResponse { docs, next })
    }

    /// Search index `index` of this context, and read every hit back from
    /// this collection. A hit whose entry is gone (the index lags state) is
    /// dropped and counted in [`Results::stale`].
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
            match self.search_entry(hit.id)? {
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

    use super::{SearchCollection, SearchError, SearchIndexSchema, Searchable};
    use crate::env;

    /// The schema of an index named `name` over `collection`.
    pub fn schema<C: SearchCollection>(_: &C, name: &str, version: u32) -> SearchIndexSchema {
        SearchIndexSchema {
            name: name.to_owned(),
            version,
            fields: <C::Value as Searchable>::search_fields(),
        }
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
///     "messages" (version = 1) => messages,
/// });
/// ```
///
/// Each entry names an index, its version, and the state field it indexes;
/// the field must be a [`SearchCollection`] whose value is
/// [`Searchable`]. Bump `version` whenever what a document holds changes
/// meaning without the declared fields changing: the node then rebuilds the
/// index from state. (A change to the fields rebuilds it anyway.)
///
/// The macro generates the three exports the node's indexer calls. It is the
/// app's whole opt-in: an app that does not use it exports none of them, and
/// the node writes nothing and runs nothing for it.
#[macro_export]
macro_rules! search_indexes {
    ($state:ty { $( $name:literal (version = $version:expr) => $field:ident ),+ $(,)? }) => {
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn __calimero_search_schema() {
            $crate::env::setup_panic_hook();
            let Some(app) = ::calimero_storage::collections::Root::<$state>::fetch() else {
                $crate::env::panic_str("Failed to find or read app state")
            };
            $crate::search::__private::respond(::core::result::Result::Ok(
                ::std::vec![$(
                    $crate::search::__private::schema(&app.$field, $name, $version)
                ),+],
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
                        $( $name => $crate::search::SearchCollection::search_extract(
                            &app.$field,
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
                        $( $name => $crate::search::SearchCollection::search_scan(
                            &app.$field,
                            request.offset,
                            request.limit,
                        ), )+
                        _ => ::core::result::Result::Ok($crate::search::ScanResponse::default()),
                    },
                ),
            );
        }
    };
}
