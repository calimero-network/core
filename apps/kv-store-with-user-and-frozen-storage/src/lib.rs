//! A key-value store with two more kinds of state beside the shared items:
//!
//! * **Per-account values** (v1's `UserStorage`): a simple value per account (`PerUser`)
//!   and a nested key-value map per account (a map keyed by `(account, key)`). **Rule: an
//!   account writes only its own.** The setters take no account argument; the value is
//!   keyed by `env::caller()`'s account, so there is no way to name anybody else's. Anyone
//!   in the org may read anyone's simple value by account (`getUserSimpleFor`). v1 keyed
//!   the writer by device for its logs; there is no device here, so events name the account.
//! * **Immutable, content-addressed values** (v1's `FrozenStorage`, here `Frozen`). **Rule:
//!   a value is stored under the SHA-256 of its content and never changed or removed**:
//!   adding the same content again is a no-op that returns the same hash. `BorshSha256`
//!   hashes the bytes v1 hashed (the value's borsh encoding: its length as a little-endian
//!   `u32`, then its UTF-8 bytes), so the hashes are v1's.
//!
//! Access: every method needs an org member (or above); guests get `Forbidden`.

use std::collections::BTreeMap;

use calimero_app_sdk::prelude::*;
use calimero_app_sdk::StorageError;
use sha2::{Digest, Sha256};

/// v1's content address: the SHA-256 of a string's borsh encoding (its length as a
/// little-endian `u32`, then its UTF-8 bytes), so the hashes are v1's.
pub struct BorshSha256;

impl ContentHash<String> for BorshSha256 {
    fn hash(value: &String) -> Result<[u8; 32], StorageError> {
        let len = u32::try_from(value.len())
            .map_err(|_| StorageError::Encode("a value over 4 GiB".into()))?;
        let mut hasher = Sha256::new();
        hasher.update(len.to_le_bytes());
        hasher.update(value.as_bytes());
        Ok(hasher.finalize().into())
    }
}

#[app::state(version = 1)]
pub struct KvStore {
    items: UnorderedMap<String, String>,
    /// Each account's simple value.
    user_simple: PerUser<String>,
    /// Each account's nested map, keyed by `(account, key)`.
    user_nested: UnorderedMap<(String, String), String>,
    frozen: Frozen<String, BorshSha256>,
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

/// The page size `entries` walks the items in.
const PAGE: u32 = 1000;

#[app::logic]
impl KvStore {
    #[app::init]
    pub fn init() -> app::Result<Self> {
        Ok(Self::load())
    }

    // --- Shared items ---

    pub fn set(&mut self, key: String, value: String) -> app::Result<()> {
        guards::require_member()?;
        let event = match self.items.insert(&key, &value)? {
            Some(_) => Event::Updated { key, value },
            None => Event::Inserted { key, value },
        };
        env::emit(event, Audience::Org)?;
        Ok(())
    }

    #[app::view]
    pub fn get(&self, key: String) -> app::Result<Option<String>> {
        guards::require_member()?;
        Ok(self.items.get(&key)?)
    }

    /// Every item, by key.
    #[app::view]
    pub fn entries(&self) -> app::Result<BTreeMap<String, String>> {
        guards::require_member()?;
        let mut out = BTreeMap::new();
        let mut opts = FindOpts::default().limit(PAGE);
        loop {
            let page = self.items.page(opts.clone())?;
            out.extend(page.items);
            match page.next {
                Some(next) => opts = opts.cursor(next),
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
        Ok(self.items.len()?)
    }

    /// The item at `key`; traps when there is none (an example of what not to do).
    #[app::view]
    pub fn get_unchecked(&self, key: String) -> app::Result<String> {
        guards::require_member()?;
        #[allow(
            clippy::expect_used,
            reason = "the trap is what this method demonstrates"
        )]
        Ok(self.items.get(&key)?.expect("key not found"))
    }

    /// The item at `key`, or `NotFound`.
    #[app::view]
    pub fn get_result(&self, key: String) -> app::Result<String> {
        guards::require_member()?;
        match self.items.get(&key)? {
            Some(value) => Ok(value),
            None => Err(Error::NotFound { key }.into()),
        }
    }

    /// Removes the item at `key`, returning its value; `Removed` only when there was one.
    pub fn remove(&mut self, key: String) -> app::Result<Option<String>> {
        guards::require_member()?;
        let old = self.items.remove(&key)?;
        if old.is_some() {
            env::emit(Event::Removed { key }, Audience::Org)?;
        }
        Ok(old)
    }

    /// Removes every item.
    pub fn clear(&mut self) -> app::Result<()> {
        guards::require_member()?;
        let _removed = self.items.clear()?;
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
        guards::require_member()?;
        let account = self.user_simple.set(&value)?;
        env::emit(Event::UserSimpleSet { account, value }, Audience::Org)?;
        Ok(())
    }

    /// The caller's own simple value.
    #[app::view]
    pub fn get_user_simple(&self) -> app::Result<Option<String>> {
        guards::require_member()?;
        self.user_simple.get()
    }

    /// `userKey`'s simple value (an account, as `myAccount` shows it).
    #[app::view]
    pub fn get_user_simple_for(&self, user_key: String) -> app::Result<Option<String>> {
        guards::require_member()?;
        Ok(self.user_simple.get_for(&user_key)?)
    }

    /// Sets `key` in the caller's own nested map.
    pub fn set_user_nested(&mut self, key: String, value: String) -> app::Result<()> {
        let account = guards::require_member()?;
        let _old = self
            .user_nested
            .insert(&(account.clone(), key.clone()), &value)?;
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
        Ok(self.user_nested.get(&(account, key))?)
    }

    // --- Frozen values: content-addressed, never changed ---

    /// Stores `value` under its content hash and returns the hash (hex). The same content
    /// again stores nothing, emits nothing and returns the same hash.
    pub fn add_frozen(&mut self, value: String) -> app::Result<String> {
        guards::require_member()?;
        let (hash, fresh) = self.frozen.insert(&value)?;
        if fresh {
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
            Some(value) => Ok(value),
            None => Err(Error::FrozenNotFound { hash }.into()),
        }
    }
}
