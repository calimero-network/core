//! A shared value and a shared map, each writable only by the accounts in its own writer
//! set (v1's `SharedStorage<T>`, re-expressed): a writer set is rows of the `writers`
//! table, and every write checks the caller against it before writing anything. The app
//! owner, who runs `init`, is the first writer of both. Anyone the app admits reads.
//!
//! `rotateWriters` replaces the writer set of the value only, so the map stays guarded by
//! the set `init` gave it; v1's end-to-end test leaned on that, and so does the scenario.

use calimero_app_sdk::env::Caller;
use calimero_app_sdk::prelude::*;

/// The guarded field a writer set belongs to.
const VALUE: &str = "value";
const MAP: &str = "map";

/// One account allowed to write one guarded field.
#[derive(Record)]
#[table(key = ["field", "account"], index(by_field = "field"), count(per_field = "field"))]
pub struct Writer {
    /// `"value"` or `"map"`.
    pub field: String,
    pub account: String,
}

/// One entry of the guarded map.
#[derive(Record)]
#[table(key = "key")]
pub struct Entry {
    pub key: String,
    pub value: String,
}

#[app::state(version = 1)]
pub struct SharedKv {
    writers: Table<Writer>,
    /// The guarded value.
    value: Cell<String>,
    map: Table<Entry>,
}

#[app::error]
pub enum Error {
    /// The caller is not in the writer set of `field`.
    NotWriter { field: String },
    /// A writer set must name at least one account.
    NoWriters,
    /// A writer set names at most `max` accounts.
    TooManyWriters { max: u32 },
}

#[app::event]
pub enum Event {
    SharedSet { value: String },
    WritersRotated { count: u32 },
}

/// The most writers one set holds.
const MAX_WRITERS: u32 = 1000;

/// The calling account, if the caller is one.
fn account() -> Option<String> {
    match env::caller() {
        Caller::Account(a) | Caller::ApiKey { account: a, .. } => Some(a),
        _ => None,
    }
}

#[app::logic]
impl SharedKv {
    /// Makes the app owner the only writer of the value and of the map.
    #[app::init]
    pub fn init() -> app::Result<Self> {
        let mut s = Self::load();
        let owner = env::app_owner();
        for field in [VALUE, MAP] {
            s.writers.insert(Writer {
                field: field.to_owned(),
                account: owner.clone(),
            })?;
        }
        Ok(s)
    }

    /// Replaces the shared value. Writers of the value only.
    pub fn set_shared(&mut self, value: String) -> app::Result<()> {
        self.require_writer(VALUE)?;
        self.value.set(&value)?;
        env::emit(Event::SharedSet { value }, Audience::All)?;
        Ok(())
    }

    /// The shared value; `""` before the first `setShared`.
    #[app::view]
    pub fn get_shared(&self) -> app::Result<String> {
        Ok(self.value.get()?.unwrap_or_default())
    }

    /// Sets an entry of the guarded map. Writers of the map only: anyone else gets
    /// `NotWriter { field: "map" }` and nothing is written.
    pub fn map_set(&mut self, key: String, value: String) -> app::Result<()> {
        self.require_writer(MAP)?;
        self.map.upsert(Entry { key, value })?;
        Ok(())
    }

    /// A map entry, or `null`.
    #[app::view]
    pub fn map_get(&self, key: String) -> app::Result<Option<String>> {
        Ok(self.map.get(&key)?.map(|e| e.value))
    }

    /// The caller's account, as a writer set names it.
    #[app::view]
    pub fn my_account(&self) -> app::Result<String> {
        account().ok_or_else(AppError::login_required)
    }

    /// Replaces the writer set of the value (not the map's) with `newWriters`. Writers of
    /// the value only.
    pub fn rotate_writers(&mut self, new_writers: Vec<String>) -> app::Result<()> {
        self.require_writer(VALUE)?;
        let count = self.replace_writers(VALUE, new_writers)?;
        env::emit(Event::WritersRotated { count }, Audience::All)?;
        Ok(())
    }

    /// How many accounts may write the value.
    #[app::view]
    pub fn writer_count(&self) -> app::Result<u64> {
        Ok(self.writers.count("perField", &[VALUE.to_key_part()])?)
    }

    /// `rotateWriters` that answers `false` instead of failing when the caller may not
    /// rotate (v1's end-to-end support method, kept for its scenario).
    pub fn try_rotate_writers(&mut self, new_writers: Vec<String>) -> app::Result<bool> {
        if self.require_writer(VALUE).is_err() {
            return Ok(false);
        }
        self.rotate_writers(new_writers)?;
        Ok(true)
    }

    fn require_writer(&self, field: &str) -> app::Result<()> {
        let allowed = match account() {
            Some(me) => self.writers.contains(&(field, me.as_str()))?,
            None => false,
        };
        if allowed {
            Ok(())
        } else {
            Err(Error::NotWriter {
                field: field.to_owned(),
            }
            .into())
        }
    }

    fn replace_writers(&mut self, field: &str, accounts: Vec<String>) -> app::Result<u32> {
        let accounts: std::collections::BTreeSet<String> = accounts.into_iter().collect();
        if accounts.is_empty() {
            return Err(Error::NoWriters.into());
        }
        let count = u32::try_from(accounts.len()).unwrap_or(u32::MAX);
        if count > MAX_WRITERS {
            return Err(Error::TooManyWriters { max: MAX_WRITERS }.into());
        }
        let old = self.writers.find(
            "byField",
            &[field.to_key_part()],
            FindOpts::default().limit(MAX_WRITERS),
        )?;
        for w in old.items {
            self.writers.remove(&(field, w.account.as_str()))?;
        }
        for account in accounts {
            self.writers.insert(Writer {
                field: field.to_owned(),
                account,
            })?;
        }
        Ok(count)
    }
}
