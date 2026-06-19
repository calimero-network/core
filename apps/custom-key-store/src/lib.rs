//! Example: using a **custom key type** in a Calimero collection.
//!
//! Collection keys are addressed by their bytes, so a key type must implement
//! `AsRef<[u8]>` (the SDK's `StorageKey` requirement). A bare numeric or
//! arbitrary struct key is rejected at compile time:
//!
//! ```ignore
//! items: UnorderedMap<u64, LwwRegister<String>>   // ERROR: `u64` can't be a
//!                                                  // collection key — not
//!                                                  // byte-encodable
//! ```
//!
//! The fix is a thin newtype that owns a byte-encodable representation and
//! forwards `AsRef<[u8]>`. `Slug` below wraps a `String` (already byte-encodable)
//! and adds domain meaning + validation, while staying a valid key. The same
//! shape works for an id newtype over `[u8; 32]`, etc.
//!
//! Everything else mirrors a normal app: CRDT state (`UnorderedMap` of
//! `LwwRegister` values), owned `String` method arguments, and `app::Result`
//! returns — all of which already satisfy the SDK's `StorageKey` / `AppArg` /
//! `AppReturn` boundaries.

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::{LwwRegister, UnorderedMap};

/// A normalized, lowercase slug used as a map key.
///
/// It is a `StorageKey` because it is borsh-(de)serializable, `PartialEq`,
/// `'static`, and — crucially — `AsRef<[u8]>` (forwarded to the inner
/// `String`). That last impl is what makes it usable as a collection key; a
/// plain struct without it would be rejected by `UnorderedMap::insert`.
#[derive(Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct Slug(String);

impl Slug {
    fn new(raw: &str) -> Self {
        Self(raw.trim().to_lowercase())
    }
}

impl AsRef<[u8]> for Slug {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

#[app::state]
pub struct CustomKeyStore {
    // The custom key type in action. `Slug: StorageKey`, so this compiles and
    // every key operation (`insert`/`get`/…) is available.
    pages: UnorderedMap<Slug, LwwRegister<String>>,
}

#[app::logic]
impl CustomKeyStore {
    #[app::init]
    pub fn init() -> CustomKeyStore {
        CustomKeyStore {
            pages: UnorderedMap::new(),
        }
    }

    /// Upsert a page body under a normalized slug. `title` is an owned `String`
    /// argument (`AppArg`-valid: `DeserializeOwned`).
    pub fn set_page(&mut self, title: String, body: String) -> app::Result<()> {
        self.pages.insert(Slug::new(&title), body.into())?;
        Ok(())
    }

    /// Fetch a page body. Returns `Option<String>` — a serializable
    /// (`AppReturn`-valid) type, encoded to JSON for the caller.
    pub fn get_page(&self, title: String) -> app::Result<Option<String>> {
        Ok(self
            .pages
            .get(&Slug::new(&title))?
            .map(|body| body.get().clone()))
    }
}
