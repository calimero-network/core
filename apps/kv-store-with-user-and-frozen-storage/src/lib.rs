//! A key-value store with two more kinds of state beside the shared items:
//!
//! * **Per-account values** (v1's `UserStorage`): a simple value and a nested key-value map
//!   per account. **Rule: an account writes only its own.** The setters take no account
//!   argument; the row is keyed by `env::caller()`'s account, so there is no way to name
//!   anybody else's. Anyone in the org may read anyone's simple value by account
//!   (`getUserSimpleFor`). v1 keyed the writer by device for its logs; there is no device
//!   here, so events name the account.
//! * **Immutable, content-addressed values** (v1's `FrozenStorage`). **Rule: a value is
//!   stored under the SHA-256 of its content and never changed or removed**: adding the
//!   same content again is a no-op that returns the same hash. The hash is computed in the
//!   guest over the bytes v1 hashed (the value's borsh encoding: its length as a
//!   little-endian `u32`, then its UTF-8 bytes), so the hashes are v1's.
//!
//! Access: every method needs an org member (or above); guests get `Forbidden`.

use std::collections::BTreeMap;

use calimero_app_sdk::prelude::*;
use sha2::{Digest, Sha256};

/// A shared item.
#[derive(Record)]
#[table(key = "key")]
pub struct Item {
    pub key: String,
    pub value: String,
}

/// An account's simple value.
#[derive(Record)]
#[table(key = "account")]
pub struct UserValue {
    pub account: String,
    pub value: String,
}

/// One entry of an account's nested map.
#[derive(Record)]
#[table(key = ["account", "key"])]
pub struct UserEntry {
    pub account: String,
    pub key: String,
    pub value: String,
}

/// An immutable value, under the hex SHA-256 of its content.
#[derive(Record)]
#[table(key = "hash")]
pub struct Frozen {
    pub hash: String,
    pub value: String,
}

#[app::state(version = 1)]
pub struct KvStore {
    items: Table<Item>,
    /// How many items there are.
    item_count: Counter,
    user_simple: Table<UserValue>,
    user_nested: Table<UserEntry>,
    frozen: Table<Frozen>,
}

#[app::error]
pub enum Error {
    /// No item at `key`.
    NotFound { key: String },
    /// No frozen value under `hash`.
    FrozenNotFound { hash: String },
    /// `hashHex` is not 64 hex digits.
    BadHash,
}

#[app::event]
pub enum Event {
    Inserted {
        key: String,
        value: String,
    },
    Updated {
        key: String,
        value: String,
    },
    Removed {
        key: String,
    },
    Cleared,
    UserSimpleSet {
        account: String,
        value: String,
    },
    UserNestedSet {
        account: String,
        key: String,
        value: String,
    },
    FrozenAdded {
        hash: String,
        value: String,
    },
}

/// The page size `entries` and `clear` walk the items in.
const PAGE: u32 = 1000;

/// The content address of `value`: the hex SHA-256 of its borsh encoding, as v1 computed it.
fn content_hash(value: &str) -> String {
    let len = u32::try_from(value.len()).unwrap_or(u32::MAX);
    let mut hasher = Sha256::new();
    hasher.update(len.to_le_bytes());
    hasher.update(value.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[app::logic]
impl KvStore {
    #[app::init]
    pub fn init() -> app::Result<Self> {
        Ok(Self::load())
    }

    // --- Shared items ---

    pub fn set(&mut self, key: String, value: String) -> app::Result<()> {
        guards::require_member()?;
        let event = if self.items.contains(&key)? {
            Event::Updated {
                key: key.clone(),
                value: value.clone(),
            }
        } else {
            self.item_count.add(1)?;
            Event::Inserted {
                key: key.clone(),
                value: value.clone(),
            }
        };
        self.items.upsert(Item { key, value })?;
        env::emit(event, Audience::Org)?;
        Ok(())
    }

    #[app::view]
    pub fn get(&self, key: String) -> app::Result<Option<String>> {
        guards::require_member()?;
        Ok(self.items.get(&key)?.map(|i| i.value))
    }

    /// Every item, by key.
    #[app::view]
    pub fn entries(&self) -> app::Result<BTreeMap<String, String>> {
        guards::require_member()?;
        let mut out = BTreeMap::new();
        let mut cursor = None;
        loop {
            let mut opts = FindOpts::default().limit(PAGE);
            if let Some(c) = cursor.take() {
                opts = opts.cursor(c);
            }
            let page = self.items.scan(opts)?;
            out.extend(page.items.into_iter().map(|i| (i.key, i.value)));
            match page.next {
                Some(next) => cursor = Some(next),
                None => return Ok(out),
            }
        }
    }

    /// How many items there are (no `isEmpty`: the ABI is v1's).
    #[allow(
        clippy::len_without_is_empty,
        reason = "an app method, not a container's"
    )]
    #[app::view]
    pub fn len(&self) -> app::Result<u64> {
        guards::require_member()?;
        Ok(self.item_count.get()?)
    }

    /// The item at `key`; traps when there is none (an example of what not to do).
    #[app::view]
    pub fn get_unchecked(&self, key: String) -> app::Result<String> {
        guards::require_member()?;
        #[allow(
            clippy::expect_used,
            reason = "the trap is what this method demonstrates"
        )]
        Ok(self.items.get(&key)?.expect("key not found").value)
    }

    /// The item at `key`, or `NotFound`.
    #[app::view]
    pub fn get_result(&self, key: String) -> app::Result<String> {
        guards::require_member()?;
        match self.items.get(&key)? {
            Some(i) => Ok(i.value),
            None => Err(Error::NotFound { key }.into()),
        }
    }

    /// Removes the item at `key`, returning its value; `Removed` only when there was one.
    pub fn remove(&mut self, key: String) -> app::Result<Option<String>> {
        guards::require_member()?;
        let old = self.items.get(&key)?;
        if old.is_some() {
            self.items.remove(&key)?;
            self.item_count.add(-1)?;
            env::emit(Event::Removed { key }, Audience::Org)?;
        }
        Ok(old.map(|i| i.value))
    }

    /// Removes every item.
    pub fn clear(&mut self) -> app::Result<()> {
        guards::require_member()?;
        loop {
            let page = self.items.scan(FindOpts::default().limit(PAGE))?;
            if page.items.is_empty() {
                break;
            }
            for item in page.items {
                self.items.remove(&item.key)?;
                self.item_count.add(-1)?;
            }
        }
        env::emit(Event::Cleared, Audience::Org)?;
        Ok(())
    }

    // --- Per-account values: each account writes only its own ---

    /// The caller's account, as `getUserSimpleFor` takes it.
    #[app::view]
    pub fn my_account(&self) -> app::Result<String> {
        guards::require_member()
    }

    /// Sets the caller's own simple value.
    pub fn set_user_simple(&mut self, value: String) -> app::Result<()> {
        let account = guards::require_member()?;
        self.user_simple.upsert(UserValue {
            account: account.clone(),
            value: value.clone(),
        })?;
        env::emit(Event::UserSimpleSet { account, value }, Audience::Org)?;
        Ok(())
    }

    /// The caller's own simple value.
    #[app::view]
    pub fn get_user_simple(&self) -> app::Result<Option<String>> {
        let account = guards::require_member()?;
        Ok(self.user_simple.get(&account)?.map(|v| v.value))
    }

    /// `userKey`'s simple value (an account, as `myAccount` shows it).
    #[app::view]
    pub fn get_user_simple_for(&self, user_key: String) -> app::Result<Option<String>> {
        guards::require_member()?;
        Ok(self.user_simple.get(&user_key)?.map(|v| v.value))
    }

    /// Sets `key` in the caller's own nested map.
    pub fn set_user_nested(&mut self, key: String, value: String) -> app::Result<()> {
        let account = guards::require_member()?;
        self.user_nested.upsert(UserEntry {
            account: account.clone(),
            key: key.clone(),
            value: value.clone(),
        })?;
        env::emit(
            Event::UserNestedSet {
                account,
                key,
                value,
            },
            Audience::Org,
        )?;
        Ok(())
    }

    /// `key` in the caller's own nested map.
    #[app::view]
    pub fn get_user_nested(&self, key: String) -> app::Result<Option<String>> {
        let account = guards::require_member()?;
        Ok(self
            .user_nested
            .get(&(account.as_str(), key.as_str()))?
            .map(|e| e.value))
    }

    // --- Frozen values: content-addressed, never changed ---

    /// Stores `value` under its content hash and returns the hash (hex). The same content
    /// again stores nothing, emits nothing and returns the same hash.
    pub fn add_frozen(&mut self, value: String) -> app::Result<String> {
        guards::require_member()?;
        let hash = content_hash(&value);
        if !self.frozen.contains(&hash)? {
            self.frozen.insert(Frozen {
                hash: hash.clone(),
                value: value.clone(),
            })?;
            env::emit(
                Event::FrozenAdded {
                    hash: hash.clone(),
                    value,
                },
                Audience::Org,
            )?;
        }
        Ok(hash)
    }

    /// The frozen value under `hashHex`.
    #[app::view]
    pub fn get_frozen(&self, hash_hex: String) -> app::Result<String> {
        guards::require_member()?;
        if hash_hex.len() != 64 || !hash_hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            app::bail!(Error::BadHash);
        }
        let hash = hash_hex.to_ascii_lowercase();
        match self.frozen.get(&hash)? {
            Some(f) => Ok(f.value),
            None => Err(Error::FrozenNotFound { hash }.into()),
        }
    }
}
