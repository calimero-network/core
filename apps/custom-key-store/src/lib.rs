//! Example: a **custom key type** in a collection.
//!
//! Storage keys are tuples of typed parts (string, int, bytes, bool, none), so a key type
//! must say which part it is by implementing `ToKeyPart` (every `ToKeyPart` is a `ToKey`,
//! the bound `UnorderedMap` and `Table::get` take). A struct that does not is rejected at
//! compile time:
//!
//! ```ignore
//! struct Point { x: f64, y: f64 }
//! pages: UnorderedMap<Point, String>   // ERROR: `Point: ToKeyPart` is not satisfied
//! ```
//!
//! The fix is a thin newtype that maps itself to a key part. `Slug` below wraps a `String`
//! and adds domain meaning (titles that differ only in case or surrounding space name one
//! page) while staying a string part, so slugs sort and compare as their strings do. The
//! same shape works for an id newtype over `[u8; 32]` (a bytes part) or a `u64` (an int part).

use calimero_app_sdk::prelude::*;

/// A normalized, lowercase slug used as a map key: one string key part.
#[derive(Clone, PartialEq, Eq)]
pub struct Slug(String);

impl Slug {
    fn new(raw: &str) -> Self {
        Self(raw.trim().to_lowercase())
    }
}

impl ToKeyPart for Slug {
    fn to_key_part(&self) -> KeyPart {
        KeyPart::Str(self.0.clone())
    }
}

#[app::state(version = 1)]
pub struct CustomKeyStore {
    /// The custom key type in action: `Slug: ToKeyPart`, so every map operation takes it.
    pages: UnorderedMap<Slug, String>,
}

#[app::logic]
impl CustomKeyStore {
    #[app::init]
    pub fn init() -> app::Result<Self> {
        Ok(Self::load())
    }

    /// Upserts a page body under the title's slug.
    pub fn set_page(&mut self, title: String, body: String) -> app::Result<()> {
        self.pages.insert(&Slug::new(&title), &body)?;
        Ok(())
    }

    /// The page body under the title's slug, or `null`.
    #[app::view]
    pub fn get_page(&self, title: String) -> app::Result<Option<String>> {
        Ok(self.pages.get(&Slug::new(&title))?)
    }
}
