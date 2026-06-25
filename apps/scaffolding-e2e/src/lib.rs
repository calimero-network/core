//! # Consolidated E2E KV Store
//!
//! A comprehensive test application that consolidates all backend E2E coverage
//! into a single app. This app exercises:
//!
//! - **KV Operations**: Basic CRUD with CRDT replication
//! - **Event Handlers**: Event-driven handlers with execution tracking
//! - **User Storage**: Per-user isolated storage (simple and nested)
//! - **Frozen Storage**: Content-addressed immutable storage
//! - **Private Storage**: Node-local private state vs replicated public state
//! - **Blob API**: Blob upload, announce, and discovery
//! - **Context Admin**: Member management
//! - **Nested CRDTs**: Complex nested CRDT compositions
//! - **RGA Document**: ReplicatedGrowableArray for text editing
//! - **Authored Map**: Shared keyspace with per-entry ownership; any member inserts, only owner mutates
//! - **Shared Storage**: Group-writable single value with rotatable writer set
//!
//! Each feature area is organized into its own method group with clear prefixes.

#![allow(clippy::len_without_is_empty)]

use std::collections::{BTreeMap, BTreeSet};

use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_sdk::{app, env, PublicKey};
use calimero_storage::collections::{
    AuthoredMap, AuthoredVector, Counter, FrozenStorage, GCounter, LwwRegister, Mergeable,
    ReplicatedGrowableArray, SharedStorage, SortedMap, SortedSet, UnorderedMap, UnorderedSet,
    UserStorage, Vector,
};
use sha2::{Digest, Sha256};
use thiserror::Error;

// CONSTANTS

const BLOB_ID_SIZE: usize = 32;
const BASE58_ENCODED_MAX_SIZE: usize = 44;

// HELPER TYPES

/// Nested map type for user storage
#[derive(Debug, BorshSerialize, BorshDeserialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
struct NestedMap {
    map: UnorderedMap<String, LwwRegister<String>>,
}

impl Mergeable for NestedMap {
    fn merge(
        &mut self,
        other: &Self,
    ) -> Result<(), calimero_storage::collections::crdt_meta::MergeError> {
        self.map.merge(&other.map)
    }
}

// `RekeyTarget` is a supertrait of `Mergeable`: this struct nests a collection
// (`map`), so it must re-key that collection's id deterministically relative to
// the entry id it is stored under, or the nested map keeps a per-replica random
// id and diverges. Delegate to the inner collection under a field-namespaced id.
impl calimero_storage::collections::rekey::RekeyTarget for NestedMap {
    fn rekey_relative_to(&mut self, parent_id: calimero_storage::address::Id) {
        calimero_storage::rekey_field_if_supported!(
            &mut self.map,
            calimero_storage::collections::rekey::field_child_id(parent_id, "map")
        );
    }

    fn register_nested_value_types() {
        calimero_storage::register_rekey_if_supported!(UnorderedMap<String, LwwRegister<String>>);
    }
}

/// File record for blob metadata
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
pub struct FileRecord {
    pub id: String,
    pub name: String,
    #[serde(serialize_with = "serialize_blob_id_bytes")]
    pub blob_id: [u8; 32],
    pub size: u64,
    pub mime_type: String,
    pub uploaded_by: String,
    pub uploaded_at: u64,
}

impl Mergeable for FileRecord {
    fn merge(
        &mut self,
        other: &Self,
    ) -> Result<(), calimero_storage::collections::crdt_meta::MergeError> {
        if other.uploaded_at > self.uploaded_at {
            *self = other.clone();
        }
        Ok(())
    }
}

// `RekeyTarget` is a supertrait of `Mergeable`. `FileRecord` is a leaf (no nested
// collection — it is merged whole-record LWW by `uploaded_at`), so the no-op
// default impl is correct: there are no nested ids to re-key.
impl calimero_storage::collections::rekey::RekeyTarget for FileRecord {
    fn rekey_relative_to(&mut self, _parent_id: calimero_storage::address::Id) {}
}

// PRIVATE STATE (Node-local, NOT synchronized)

#[derive(BorshSerialize, BorshDeserialize, Debug)]
#[borsh(crate = "calimero_sdk::borsh")]
#[app::private]
pub struct PrivateSecrets {
    secrets: UnorderedMap<String, String>,
}

impl Default for PrivateSecrets {
    fn default() -> Self {
        Self {
            secrets: UnorderedMap::new(),
        }
    }
}

// MAIN STATE

#[app::state(emits = for<'a> Event<'a>)]
pub struct E2eKvStore {
    // --- KV Storage ---
    /// Public replicated KV map
    kv_items: UnorderedMap<String, LwwRegister<String>>,

    // --- Handler Tracking ---
    /// Counter for handler executions (CRDT G-Counter - grow only)
    handler_counter: Counter,

    // --- User Storage ---
    /// Simple user-owned data (e.g., profile name)
    user_items_simple: UserStorage<LwwRegister<String>>,
    /// Nested user-owned data (e.g., user's private KV store)
    user_items_nested: UserStorage<NestedMap>,

    // --- Frozen Storage ---
    /// Content-addressed immutable storage
    frozen_items: FrozenStorage<String>,

    // --- Private Game (public hash tracking) ---
    /// Maps game_id -> SHA256(secret) hex
    games: UnorderedMap<String, LwwRegister<String>>,

    // --- Blob Storage ---
    /// File metadata records
    files: UnorderedMap<String, FileRecord>,
    /// Counter for generating file IDs
    file_counter: LwwRegister<u64>,
    /// Owner of the file share context
    file_owner: LwwRegister<String>,

    // --- Nested CRDTs ---
    /// Map of G-Counters (grow-only, concurrent increments should sum)
    /// Counter<false> = GCounter (default)
    crdt_counters: UnorderedMap<String, Counter>,
    /// Map of PN-Counters (supports decrement, concurrent inc/dec should merge correctly)
    /// Counter<true> = PNCounter (allows decrement)
    crdt_pn_counters: UnorderedMap<String, Counter<true>>,
    /// Map of LWW registers (latest timestamp wins)
    crdt_registers: UnorderedMap<String, LwwRegister<String>>,
    /// Nested maps (field-level merge)
    crdt_metadata: UnorderedMap<String, UnorderedMap<String, LwwRegister<String>>>,
    /// Vector of G-Counters (element-wise merge)
    crdt_metrics: Vector<Counter>,
    /// Map of sets (union merge)
    crdt_tags: UnorderedMap<String, UnorderedSet<String>>,

    // --- Sorted Map (key-ordered; range/prefix/pagination) ---
    /// Key-ordered map exercised end-to-end through the WASM host index path.
    sorted_items: SortedMap<String, LwwRegister<String>>,

    // --- Sorted Set (element-ordered; range/membership/min-max) ---
    /// Element-ordered set, exercised end-to-end through the same WASM host
    /// ordered-index path as `sorted_items` (the `SortedSet` counterpart).
    sorted_tags: SortedSet<String>,

    // --- RGA Document ---
    /// Collaborative text document
    rga_document: ReplicatedGrowableArray,
    /// Edit count for document (G-Counter)
    rga_edit_count: Counter,
    /// Document metadata (title, owner)
    rga_metadata: UnorderedMap<String, LwwRegister<String>>,

    // --- Authored Map ---
    /// Shared keyspace map with per-entry ownership
    authored_items: AuthoredMap<String, LwwRegister<String>>,

    // --- Authored Vector ---
    /// Append-only vector with per-slot ownership; only the pusher can update/tombstone their slot
    authored_vec: AuthoredVector<LwwRegister<String>>,

    // --- Shared Storage ---
    /// Group-writable single value; writers rotate at runtime
    shared_data: SharedStorage<LwwRegister<String>>,
}

// EVENTS

#[app::event]
pub enum Event<'a> {
    // KV Events
    Inserted {
        key: &'a str,
        value: &'a str,
    },
    Updated {
        key: &'a str,
        value: &'a str,
    },
    Removed {
        key: &'a str,
    },
    Cleared,

    // User Storage Events
    UserSimpleSet {
        executor_id: PublicKey,
        value: &'a str,
    },
    UserNestedSet {
        executor_id: PublicKey,
        key: &'a str,
        value: &'a str,
    },

    // Frozen Storage Events
    FrozenAdded {
        hash: [u8; 32],
        value: &'a str,
    },

    // Private Game Events
    SecretSet {
        game_id: &'a str,
    },
    Guessed {
        game_id: &'a str,
        success: bool,
        by: &'a str,
    },

    // Blob Events
    FileUploaded {
        id: String,
        name: String,
        size: u64,
        uploader: String,
    },
    FileDeleted {
        id: String,
        name: String,
    },

    // Nested CRDT Events
    GCounterIncremented {
        key: String,
        value: u64,
    },
    PnCounterChanged {
        key: String,
        value: i64,
        operation: &'a str,
    },
    RegisterSet {
        key: String,
        value: String,
    },
    MetadataSet {
        outer_key: String,
        inner_key: String,
        value: String,
    },
    MetricPushed {
        value: u64,
    },
    TagAdded {
        key: String,
        tag: String,
    },

    // RGA Events
    DocumentCreated {
        title: String,
        owner: String,
    },
    TextInserted {
        position: usize,
        text: String,
        editor: String,
    },
    TextDeleted {
        start: usize,
        end: usize,
        editor: String,
    },
    TitleChanged {
        old_title: String,
        new_title: String,
        editor: String,
    },

    // Authored Map Events
    AuthoredInserted {
        key: String,
        value: String,
        owner: String,
    },
    AuthoredUpdated {
        key: String,
        value: String,
    },
    AuthoredRemoved {
        key: String,
    },

    // Authored Vector Events
    AuthoredVecPushed {
        index: usize,
        value: String,
        owner: String,
    },
    AuthoredVecUpdated {
        index: usize,
        value: String,
    },
    AuthoredVecRemoved {
        index: usize,
    },

    // Shared Storage Events
    SharedSet {
        value: String,
        by: String,
    },
    SharedWriterAdded {
        writer: String,
    },
}

// ERRORS

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Error<'a> {
    #[error("key not found: {0}")]
    NotFound(&'a str),
    #[error("user data not found for key: {0}")]
    UserNotFound(PublicKey),
    #[error("frozen data not found for hash: {0}")]
    FrozenNotFound(&'a str),
    #[error("no public hash set yet")]
    NoHash,
}

// HELPER FUNCTIONS

fn encode_identity(identity: &[u8; 32]) -> String {
    bs58::encode(identity).into_string()
}

fn encode_blob_id_base58(blob_id_bytes: &[u8; BLOB_ID_SIZE]) -> String {
    let mut buf = [0u8; BASE58_ENCODED_MAX_SIZE];
    // Both unwraps are infallible for this input: a 32-byte value base58-encodes
    // to at most 44 chars (== BASE58_ENCODED_MAX_SIZE), so `onto` never overflows
    // the buffer, and the base58 alphabet is ASCII, so the bytes are always UTF-8.
    let len = bs58::encode(blob_id_bytes).onto(&mut buf[..]).unwrap();
    std::str::from_utf8(&buf[..len]).unwrap().to_owned()
}

fn parse_blob_id_base58(blob_id_str: &str) -> app::Result<[u8; BLOB_ID_SIZE]> {
    let bytes = bs58::decode(blob_id_str)
        .into_vec()
        .map_err(|e| app::err!("Failed to decode blob ID '{blob_id_str}': {e}"))?;

    if bytes.len() != BLOB_ID_SIZE {
        app::bail!(
            "Invalid blob ID length: expected {} bytes, got {}",
            BLOB_ID_SIZE,
            bytes.len()
        );
    }

    let mut blob_id = [0u8; BLOB_ID_SIZE];
    blob_id.copy_from_slice(&bytes);
    Ok(blob_id)
}

fn serialize_blob_id_bytes<S>(
    blob_id_bytes: &[u8; BLOB_ID_SIZE],
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: calimero_sdk::serde::Serializer,
{
    let safe_string = encode_blob_id_base58(blob_id_bytes);
    serializer.serialize_str(&safe_string)
}

// APPLICATION LOGIC

#[app::logic]
impl E2eKvStore {
    // INITIALIZATION

    #[app::init]
    pub fn init() -> E2eKvStore {
        app::log!("Initializing E2E KV Store");

        E2eKvStore {
            // KV
            kv_items: UnorderedMap::new(),
            // Handlers
            handler_counter: Counter::new(),
            // User Storage
            user_items_simple: UserStorage::new(),
            user_items_nested: UserStorage::new(),
            // Frozen Storage
            frozen_items: FrozenStorage::new(),
            // Private Game
            games: UnorderedMap::new(),
            // Blob Storage
            files: UnorderedMap::new(),
            file_counter: LwwRegister::new(0),
            file_owner: LwwRegister::new(String::new()),
            // Nested CRDTs
            crdt_counters: UnorderedMap::new(),
            crdt_pn_counters: UnorderedMap::new(),
            crdt_registers: UnorderedMap::new(),
            crdt_metadata: UnorderedMap::new(),
            crdt_metrics: Vector::new(),
            crdt_tags: UnorderedMap::new(),
            sorted_items: SortedMap::new(),
            sorted_tags: SortedSet::new(),
            // RGA
            rga_document: ReplicatedGrowableArray::new(),
            rga_edit_count: GCounter::new(),
            rga_metadata: UnorderedMap::new(),
            // Authored Map
            authored_items: AuthoredMap::new(),
            // Authored Vector
            authored_vec: AuthoredVector::<LwwRegister<String>>::new(),
            // Shared Storage — init caller becomes the sole initial writer
            shared_data: SharedStorage::new(
                std::iter::once(env::executor_id().into()).collect(),
                false,
            ),
        }
    }

    // KV OPERATIONS

    /// Basic KV set without handlers
    pub fn set(&mut self, key: String, value: String) -> app::Result<()> {
        app::log!("Setting key: {:?} to value: {:?}", key, value);

        if self.kv_items.contains(&key)? {
            app::emit!(Event::Updated {
                key: &key,
                value: &value
            });
        } else {
            app::emit!(Event::Inserted {
                key: &key,
                value: &value
            });
        }

        self.kv_items.insert(key, value.into())?;
        Ok(())
    }

    /// Test-only probe for the host-backed `tracing` subscriber.
    ///
    /// Emits one `tracing` event at each level and performs a KV insert so the
    /// storage crate's *own* `tracing` output (e.g. `Interface::apply_action`,
    /// `commit_root`) is exercised through the same path. When `debug` is set,
    /// raises the level to DEBUG first so those lower-level lines pass the
    /// filter. Driven by the runtime `tracing_logs` integration test; the
    /// convergence tests never call it, keeping DEBUG spam out of them.
    pub fn tracing_probe(&mut self, debug: bool) -> app::Result<()> {
        if debug {
            env::set_log_level(env::LevelFilter::DEBUG);
        }
        tracing::info!(probe = true, "tracing_probe: info line");
        tracing::debug!(probe = true, "tracing_probe: debug line");
        tracing::warn!("tracing_probe: warn line");
        // Mutating storage drives the storage crate's internal `tracing`.
        self.kv_items
            .insert("probe_key".to_owned(), "probe_value".to_owned().into())?;
        Ok(())
    }

    /// KV set with handler triggers (for testing event-driven handlers)
    pub fn set_with_handler(&mut self, key: String, value: String) -> app::Result<()> {
        app::log!("Setting key with handler: {:?} to value: {:?}", key, value);

        if self.kv_items.contains(&key)? {
            app::emit!((
                Event::Updated {
                    key: &key,
                    value: &value
                },
                "update_handler"
            ));
        } else {
            app::emit!((
                Event::Inserted {
                    key: &key,
                    value: &value
                },
                "insert_handler"
            ));
        }

        self.kv_items.insert(key, value.into())?;
        Ok(())
    }

    pub fn get(&self, key: &str) -> app::Result<Option<String>> {
        app::log!("Getting key: {:?}", key);
        Ok(self.kv_items.get(key)?.map(|v| v.get().clone()))
    }

    pub fn get_result(&self, key: &str) -> app::Result<String> {
        app::log!("Getting key, possibly failing: {:?}", key);
        let Some(value) = self.get(key)? else {
            app::bail!(Error::NotFound(key));
        };
        Ok(value)
    }

    pub fn entries(&self) -> app::Result<BTreeMap<String, String>> {
        app::log!("Getting all entries");
        Ok(self
            .kv_items
            .entries()?
            .map(|(k, v)| (k, v.get().clone()))
            .collect())
    }

    // --- SortedMap (key-ordered) operations ---
    //
    // These drive the WASM host ordered-index path end to end: `sorted_set`
    // maintains the index via `storage_index_set`; `sorted_keys`/`sorted_range`
    // read it back via `storage_index_scan`.

    pub fn sorted_set(&mut self, key: String, value: String) -> app::Result<()> {
        self.sorted_items.insert(key, value.into())?;
        Ok(())
    }

    /// All keys in ascending order (index-backed).
    pub fn sorted_keys(&self) -> app::Result<Vec<String>> {
        Ok(self.sorted_items.keys()?.collect())
    }

    /// Entries whose keys fall within `[start, end)`, ascending (a range seek).
    pub fn sorted_range(
        &self,
        start: String,
        end: String,
    ) -> app::Result<BTreeMap<String, String>> {
        Ok(self
            .sorted_items
            .range(start..end)?
            .map(|(k, v)| (k, v.get().clone()))
            .collect())
    }

    /// The largest key (reverse-seek `last`, index-backed).
    pub fn sorted_last_key(&self) -> app::Result<Option<String>> {
        Ok(self.sorted_items.last()?.map(|(k, _)| k))
    }

    // --- SortedSet (element-ordered) operations ---
    //
    // The `SortedSet` counterpart of the methods above: `sorted_tag_add`
    // maintains the ordered index, `sorted_tags_all`/`sorted_tags_range` read it
    // back in element order, all through the same WASM host index path.

    /// Insert `tag`; returns `true` if it was newly added.
    pub fn sorted_tag_add(&mut self, tag: String) -> app::Result<bool> {
        Ok(self.sorted_tags.insert(tag)?)
    }

    /// Remove `tag`; returns `true` if it was present.
    pub fn sorted_tag_remove(&mut self, tag: String) -> app::Result<bool> {
        Ok(self.sorted_tags.remove(&tag)?)
    }

    /// Whether `tag` is in the set.
    pub fn sorted_tag_contains(&self, tag: String) -> app::Result<bool> {
        Ok(self.sorted_tags.contains(&tag)?)
    }

    /// All elements in ascending order (index-backed).
    pub fn sorted_tags_all(&self) -> app::Result<Vec<String>> {
        Ok(self.sorted_tags.iter()?.collect())
    }

    /// Elements within `[start, end)`, ascending (a range seek).
    pub fn sorted_tags_range(&self, start: String, end: String) -> app::Result<Vec<String>> {
        Ok(self.sorted_tags.range(start..end)?.collect())
    }

    /// The largest element (reverse-seek `last`, index-backed).
    pub fn sorted_tags_last(&self) -> app::Result<Option<String>> {
        self.sorted_tags.last().map_err(Into::into)
    }

    pub fn len(&self) -> app::Result<usize> {
        app::log!("Getting the number of entries");
        Ok(self.kv_items.len()?)
    }

    pub fn remove(&mut self, key: &str) -> app::Result<Option<String>> {
        app::log!("Removing key: {:?}", key);
        app::emit!(Event::Removed { key });
        Ok(self.kv_items.remove(key)?.map(|v| v.get().clone()))
    }

    pub fn clear(&mut self) -> app::Result<()> {
        app::log!("Clearing all entries");
        app::emit!(Event::Cleared);
        self.kv_items.clear().map_err(Into::into)
    }

    /// Remove with handler trigger (for testing event-driven handlers)
    pub fn remove_with_handler(&mut self, key: &str) -> app::Result<Option<String>> {
        app::log!("Removing key with handler: {:?}", key);
        app::emit!((Event::Removed { key }, "remove_handler"));
        Ok(self.kv_items.remove(key)?.map(|v| v.get().clone()))
    }

    /// Clear with handler trigger (for testing event-driven handlers)
    pub fn clear_with_handler(&mut self) -> app::Result<()> {
        app::log!("Clearing all entries with handler");
        app::emit!((Event::Cleared, "clear_handler"));
        self.kv_items.clear().map_err(Into::into)
    }

    // EVENT HANDLERS

    pub fn insert_handler(&mut self, key: &str, value: &str) -> app::Result<()> {
        app::log!(
            "Handler 'insert_handler' called: key={}, value={}",
            key,
            value
        );
        self.handler_counter.increment()?;
        Ok(())
    }

    pub fn update_handler(&mut self, key: &str, value: &str) -> app::Result<()> {
        app::log!(
            "Handler 'update_handler' called: key={}, value={}",
            key,
            value
        );
        self.handler_counter.increment()?;
        Ok(())
    }

    pub fn remove_handler(&mut self, key: &str) -> app::Result<()> {
        app::log!("Handler 'remove_handler' called: key={}", key);
        self.handler_counter.increment()?;
        Ok(())
    }

    pub fn clear_handler(&mut self) -> app::Result<()> {
        app::log!("Handler 'clear_handler' called: all items cleared");
        self.handler_counter.increment()?;
        Ok(())
    }

    pub fn get_handler_execution_count(&self) -> app::Result<u64> {
        Ok(self.handler_counter.value()?)
    }

    // USER STORAGE - SIMPLE

    pub fn set_user_simple(&mut self, value: String) -> app::Result<()> {
        let executor_id = env::executor_id();
        app::log!(
            "Setting simple value for user {:?}: {:?}",
            executor_id,
            value
        );
        app::emit!(Event::UserSimpleSet {
            executor_id: executor_id.into(),
            value: &value
        });
        self.user_items_simple.insert(value.into())?;
        Ok(())
    }

    pub fn get_user_simple(&self) -> app::Result<Option<String>> {
        let executor_id = env::executor_id();
        app::log!("Getting simple value for user {:?}", executor_id);
        Ok(self.user_items_simple.get()?.map(|v| v.get().clone()))
    }

    pub fn get_user_simple_for(&self, user_key: PublicKey) -> app::Result<Option<String>> {
        app::log!("Getting simple value for specific user {:?}", user_key);
        Ok(self
            .user_items_simple
            .get_for_user(&user_key)?
            .map(|v| v.get().clone()))
    }

    // USER STORAGE - NESTED

    pub fn set_user_nested(&mut self, key: String, value: String) -> app::Result<()> {
        let executor_id = env::executor_id();
        app::log!(
            "Setting nested key {:?} for user {:?}: {:?}",
            key,
            executor_id,
            value
        );

        let mut nested_map = self.user_items_nested.get()?.unwrap_or_default();
        nested_map.map.insert(key.clone(), value.clone().into())?;
        self.user_items_nested.insert(nested_map)?;

        app::emit!(Event::UserNestedSet {
            executor_id: executor_id.into(),
            key: &key,
            value: &value
        });
        Ok(())
    }

    pub fn get_user_nested(&self, key: &str) -> app::Result<Option<String>> {
        let executor_id = env::executor_id();
        app::log!("Getting nested key {:?} for user {:?}", key, executor_id);

        let nested_map = self.user_items_nested.get()?;
        match nested_map {
            Some(map) => Ok(map.map.get(key)?.map(|v| v.get().clone())),
            None => Ok(None),
        }
    }

    // FROZEN STORAGE

    pub fn add_frozen(&mut self, value: String) -> app::Result<String> {
        app::log!("Adding frozen value: {:?}", value);

        let hash = self.frozen_items.insert(value.clone())?;

        app::emit!(Event::FrozenAdded {
            hash,
            value: &value
        });

        let hash_hex = hex::encode(hash);
        Ok(hash_hex)
    }

    pub fn get_frozen(&self, hash_hex: String) -> app::Result<String> {
        app::log!("Getting frozen value for hash {:?}", hash_hex);
        let mut hash = [0u8; 32];
        hex::decode_to_slice(&hash_hex, &mut hash[..])
            .map_err(|_| Error::NotFound("dehex error"))?;

        Ok(self
            .frozen_items
            .get(&hash)?
            .ok_or(Error::FrozenNotFound("Frozen value is not found"))?)
    }

    // PRIVATE STORAGE

    pub fn add_secret(&mut self, game_id: String, secret: String) -> app::Result<()> {
        // Save private secret using private storage
        let mut secrets = PrivateSecrets::private_load_or_default()?;
        let mut secrets_mut = secrets.as_mut();
        secrets_mut
            .secrets
            .insert(game_id.clone(), secret.clone())?;

        // Save public hash for guess verification
        let hash = Sha256::digest(secret.as_bytes());
        let hash_hex = hex::encode(hash);
        self.games.insert(game_id.clone(), hash_hex.into())?;
        app::emit!(Event::SecretSet { game_id: &game_id });
        Ok(())
    }

    pub fn add_guess(&self, game_id: &str, guess: String) -> app::Result<bool> {
        let Some(public_hash_hex) = self.games.get(game_id)?.map(|v| v.get().clone()) else {
            app::bail!(Error::NoHash);
        };
        let guess_hash = Sha256::digest(guess.as_bytes());
        let guess_hash_hex = hex::encode(guess_hash);
        let who_b = env::executor_id();
        let who = bs58::encode(who_b).into_string();
        let success = guess_hash_hex == public_hash_hex;
        app::emit!(Event::Guessed {
            game_id,
            success,
            by: &who
        });
        Ok(success)
    }

    pub fn my_secrets(&self) -> app::Result<BTreeMap<String, String>> {
        let secrets = PrivateSecrets::private_load_or_default()?;
        let map: BTreeMap<_, _> = secrets.secrets.entries()?.collect();
        Ok(map)
    }

    pub fn games(&self) -> app::Result<BTreeMap<String, String>> {
        Ok(self
            .games
            .entries()?
            .map(|(k, v)| (k, v.get().clone()))
            .collect())
    }

    // BLOB API

    pub fn upload_file(
        &mut self,
        name: String,
        blob_id_str: String,
        size: u64,
        mime_type: String,
    ) -> app::Result<String> {
        let blob_id = parse_blob_id_base58(&blob_id_str)?;

        let current_counter = *self.file_counter.get();
        let file_id = format!("file_{current_counter}");
        self.file_counter.set(current_counter + 1);

        let uploader_id = env::executor_id();
        let uploader = encode_blob_id_base58(&uploader_id);
        let timestamp = env::time_now();

        // Announce blob to network for peer discovery
        let current_context = env::context_id();
        if env::blob_announce_to_context(&blob_id, &current_context) {
            app::log!("Announced blob {} to network", blob_id_str);
        } else {
            app::log!("Warning: Failed to announce blob {}", blob_id_str);
        }

        let file_record = FileRecord {
            id: file_id.clone(),
            name: name.clone(),
            blob_id,
            size,
            mime_type,
            uploaded_by: uploader.clone(),
            uploaded_at: timestamp,
        };

        self.files.insert(file_id.clone(), file_record)?;

        app::emit!(Event::FileUploaded {
            id: file_id.clone(),
            name: name.clone(),
            size,
            uploader,
        });

        app::log!("File uploaded successfully: {} (ID: {})", name, file_id);
        Ok(file_id)
    }

    pub fn delete_file(&mut self, file_id: String) -> app::Result<()> {
        let file_record = self
            .files
            .get(&file_id)?
            .ok_or_else(|| app::err!("File not found: {file_id}"))?;

        let file_name = file_record.name.clone();

        self.files.remove(&file_id)?;

        app::emit!(Event::FileDeleted {
            id: file_id.clone(),
            name: file_name.clone(),
        });

        app::log!("File deleted: {} (ID: {})", file_name, file_id);
        Ok(())
    }

    pub fn list_files(&self) -> app::Result<Vec<FileRecord>> {
        let mut files = Vec::new();
        for (_, file_record) in self.files.entries()? {
            files.push(file_record.clone());
        }
        app::log!("Listed {} files", files.len());
        Ok(files)
    }

    pub fn get_file(&self, file_id: String) -> app::Result<FileRecord> {
        let Some(file_record) = self.files.get(&file_id)? else {
            app::bail!("File not found: {file_id}");
        };

        Ok(file_record.clone())
    }

    pub fn get_blob_id_b58(&self, file_id: String) -> app::Result<String> {
        let file_record = self.get_file(file_id)?;
        Ok(encode_blob_id_base58(&file_record.blob_id))
    }

    pub fn search_files(&self, query: String) -> app::Result<Vec<FileRecord>> {
        let mut results = Vec::new();
        let query_lower = query.to_lowercase();

        for (_, file_record) in self.files.entries()? {
            if file_record.name.to_lowercase().contains(&query_lower) {
                results.push(file_record.clone());
            }
        }

        app::log!("Search for '{}' found {} results", query, results.len());
        Ok(results)
    }

    // NESTED CRDT - COUNTERS

    // --- G-COUNTER (grow-only) ---

    pub fn increment_g_counter(&mut self, key: String) -> app::Result<u64> {
        let mut counter = self.crdt_counters.entry(key.clone())?.or_default()?;

        counter.increment()?;

        let value = counter.value()?;

        app::emit!(Event::GCounterIncremented { key, value });
        Ok(value)
    }

    pub fn get_g_counter(&self, key: String) -> app::Result<u64> {
        let Some(counter) = self.crdt_counters.get(&key)? else {
            app::bail!("GCounter not found");
        };

        Ok(counter.value()?)
    }

    // --- PN-COUNTER (supports increment AND decrement) ---

    pub fn increment_pn_counter(&mut self, key: String) -> app::Result<i64> {
        let mut counter = self.crdt_pn_counters.entry(key.clone())?.or_default()?;

        counter.increment()?;

        let value = counter.value()?;

        app::emit!(Event::PnCounterChanged {
            key,
            value,
            operation: "increment"
        });
        Ok(value)
    }

    pub fn decrement_pn_counter(&mut self, key: String) -> app::Result<i64> {
        let mut counter = self.crdt_pn_counters.entry(key.clone())?.or_default()?;

        counter.decrement()?;

        let value = counter.value()?;

        app::emit!(Event::PnCounterChanged {
            key,
            value,
            operation: "decrement"
        });
        Ok(value)
    }

    pub fn get_pn_counter(&self, key: String) -> app::Result<i64> {
        let Some(counter) = self.crdt_pn_counters.get(&key)? else {
            app::bail!("PNCounter not found");
        };

        Ok(counter.value()?)
    }

    // Legacy alias for backward compatibility
    pub fn increment_counter(&mut self, key: String) -> app::Result<u64> {
        self.increment_g_counter(key)
    }

    pub fn get_counter(&self, key: String) -> app::Result<u64> {
        self.get_g_counter(key)
    }

    // NESTED CRDT - REGISTERS

    pub fn set_register(&mut self, key: String, value: String) -> app::Result<()> {
        let register = LwwRegister::new(value.clone());

        self.crdt_registers.insert(key.clone(), register)?;

        app::emit!(Event::RegisterSet { key, value });
        Ok(())
    }

    pub fn get_register(&self, key: String) -> app::Result<String> {
        self.crdt_registers
            .get(&key)?
            .map(|r| r.get().clone())
            .ok_or_else(|| app::err!("Register not found"))
    }

    // NESTED CRDT - METADATA

    pub fn set_metadata(
        &mut self,
        outer_key: String,
        inner_key: String,
        value: String,
    ) -> app::Result<()> {
        let mut inner_map = self.crdt_metadata.entry(outer_key.clone())?.or_default()?;

        inner_map.insert(inner_key.clone(), value.clone().into())?;

        app::emit!(Event::MetadataSet {
            outer_key,
            inner_key,
            value,
        });
        Ok(())
    }

    pub fn get_metadata(&self, outer_key: String, inner_key: String) -> app::Result<String> {
        self.crdt_metadata
            .get(&outer_key)?
            .ok_or_else(|| app::err!("Outer key not found"))?
            .get(&inner_key)?
            .ok_or_else(|| app::err!("Inner key not found"))
            .map(|v| v.get().clone())
    }

    // NESTED CRDT - METRICS VECTOR

    pub fn push_metric(&mut self, value: u64) -> app::Result<usize> {
        let mut counter = GCounter::new();
        for _ in 0..value {
            counter.increment()?;
        }

        self.crdt_metrics.push(counter)?;

        let len = self.crdt_metrics.len()?;

        app::emit!(Event::MetricPushed { value });
        Ok(len)
    }

    pub fn get_metric(&self, index: usize) -> app::Result<u64> {
        self.crdt_metrics
            .get(index)?
            .ok_or_else(|| app::err!("Index out of bounds"))?
            .value()
            .map_err(Into::into)
    }

    pub fn metrics_len(&self) -> app::Result<usize> {
        self.crdt_metrics.len().map_err(Into::into)
    }

    // NESTED CRDT - TAGS SET

    pub fn add_tag(&mut self, key: String, tag: String) -> app::Result<()> {
        let mut set = self.crdt_tags.entry(key.clone())?.or_default()?;

        set.insert(tag.clone())?;

        app::emit!(Event::TagAdded { key, tag });
        Ok(())
    }

    pub fn has_tag(&self, key: String, tag: String) -> app::Result<bool> {
        let Some(set) = self.crdt_tags.get(&key)? else {
            app::bail!("Key not found");
        };

        Ok(set.contains(&tag)?)
    }

    pub fn get_tag_count(&self, key: String) -> app::Result<u64> {
        let count = self
            .crdt_tags
            .get(&key)?
            .ok_or_else(|| app::err!("Key not found"))?
            .iter()?
            .count();

        Ok(count as u64)
    }

    // RGA DOCUMENT (from collaborative-editor)

    pub fn rga_insert_text(&mut self, position: usize, text: String) -> app::Result<()> {
        let editor_id = env::executor_id();
        let editor = encode_identity(&editor_id);

        app::log!(
            "Inserting '{}' at position {} by {}",
            text,
            position,
            editor
        );

        self.rga_document.insert_str(position, &text)?;

        self.rga_edit_count.increment()?;

        app::emit!(Event::TextInserted {
            position,
            text: text.clone(),
            editor,
        });

        Ok(())
    }

    pub fn rga_delete_text(&mut self, start: usize, end: usize) -> app::Result<()> {
        let editor_id = env::executor_id();
        let editor = encode_identity(&editor_id);

        app::log!("Deleting text from {} to {} by {}", start, end, editor);

        self.rga_document.delete_range(start, end)?;

        self.rga_edit_count.increment()?;

        app::emit!(Event::TextDeleted { start, end, editor });

        Ok(())
    }

    pub fn rga_get_text(&self) -> app::Result<String> {
        self.rga_document.get_text().map_err(Into::into)
    }

    pub fn rga_get_length(&self) -> app::Result<usize> {
        self.rga_document.len().map_err(Into::into)
    }

    pub fn rga_is_empty(&self) -> app::Result<bool> {
        self.rga_document.is_empty().map_err(Into::into)
    }

    pub fn rga_set_title(&mut self, new_title: String) -> app::Result<()> {
        if new_title.is_empty() {
            app::bail!("Title cannot be empty");
        }

        let editor_id = env::executor_id();
        let editor = encode_identity(&editor_id);

        let old_title = self.rga_get_title();

        self.rga_metadata
            .insert("title".to_string(), new_title.clone().into())?;

        app::log!(
            "Title changed from '{}' to '{}' by {}",
            old_title,
            new_title,
            editor
        );

        app::emit!(Event::TitleChanged {
            old_title,
            new_title,
            editor,
        });

        Ok(())
    }

    pub fn rga_get_title(&self) -> String {
        self.rga_metadata
            .get("title")
            .ok()
            .flatten()
            .map(|v| v.get().clone())
            .unwrap_or_else(|| "Untitled Document".to_string())
    }

    pub fn rga_append_text(&mut self, text: String) -> app::Result<()> {
        let length = self.rga_get_length()?;
        self.rga_insert_text(length, text)
    }

    pub fn rga_clear(&mut self) -> app::Result<()> {
        let length = self.rga_get_length()?;
        if length > 0 {
            self.rga_delete_text(0, length)?;
        }
        Ok(())
    }

    // AUTHORED MAP

    pub fn authored_insert(&mut self, key: String, value: String) -> app::Result<()> {
        let owner = bs58::encode(env::executor_id()).into_string();
        self.authored_items
            .insert(key.clone(), value.clone().into())?;
        app::emit!(Event::AuthoredInserted {
            key: key.clone(),
            value: value.clone(),
            owner: owner.clone(),
        });
        Ok(())
    }

    pub fn authored_update(&mut self, key: String, value: String) -> app::Result<()> {
        self.authored_items.update(&key, value.clone().into())?;
        app::emit!(Event::AuthoredUpdated {
            key: key.clone(),
            value: value.clone(),
        });
        Ok(())
    }

    pub fn authored_remove(&mut self, key: String) -> app::Result<Option<String>> {
        let result = self.authored_items.remove(&key)?.map(|v| v.get().clone());
        if result.is_some() {
            app::emit!(Event::AuthoredRemoved { key: key.clone() });
        }
        Ok(result)
    }

    pub fn authored_get(&self, key: String) -> app::Result<Option<String>> {
        Ok(self.authored_items.get(&key)?.map(|v| v.get().clone()))
    }

    pub fn authored_entries(&self) -> app::Result<BTreeMap<String, String>> {
        Ok(self
            .authored_items
            .entries()?
            .map(|(k, v)| (k, v.get().clone()))
            .collect())
    }

    pub fn authored_get_owner(&self, key: String) -> app::Result<Option<String>> {
        Ok(self.authored_items.owner_of(&key)?.map(|pk| pk.to_string()))
    }

    pub fn authored_len(&self) -> app::Result<usize> {
        self.authored_items.len().map_err(Into::into)
    }

    // SHARED STORAGE

    pub fn shared_set(&mut self, value: String) -> app::Result<()> {
        let by = bs58::encode(env::executor_id()).into_string();
        self.shared_data.insert(LwwRegister::new(value.clone()))?;
        app::emit!(Event::SharedSet {
            value: value.clone(),
            by: by.clone(),
        });
        Ok(())
    }

    pub fn shared_get(&self) -> app::Result<String> {
        Ok(self.shared_data.get()?.get().clone())
    }

    pub fn shared_get_writers(&self) -> app::Result<Vec<String>> {
        Ok(self
            .shared_data
            .writers()
            .iter()
            .map(|pk| pk.to_string())
            .collect())
    }

    pub fn shared_add_writer(&mut self, writer_bs58: String) -> app::Result<()> {
        let new_writer: PublicKey = writer_bs58.parse()?;
        let mut new_writers = self.shared_data.writers().clone();
        new_writers.insert(new_writer);
        self.shared_data.rotate_writers(new_writers)?;
        app::emit!(Event::SharedWriterAdded {
            writer: writer_bs58.clone(),
        });
        Ok(())
    }

    /// Replace the entire writer set in one rotation (unlike `shared_add_writer`,
    /// which only unions in a single key). Lets a test drop a writer — required
    /// to exercise concurrent rotations that diverge on membership and to assert
    /// retroactive revocation. The caller must be a current writer.
    pub fn shared_rotate_writers(&mut self, writers: Vec<String>) -> app::Result<()> {
        let mut new_writers = BTreeSet::new();
        for w in &writers {
            let pk: PublicKey = w.parse()?;
            let _inserted = new_writers.insert(pk);
        }
        self.shared_data.rotate_writers(new_writers)?;
        for w in writers {
            app::emit!(Event::SharedWriterAdded { writer: w });
        }
        Ok(())
    }

    pub fn shared_is_writer(&self, key_bs58: String) -> app::Result<bool> {
        let pk: PublicKey = key_bs58.parse()?;
        Ok(self.shared_data.writers().contains(&pk))
    }

    pub fn shared_is_frozen(&self) -> app::Result<bool> {
        Ok(self.shared_data.is_frozen())
    }

    // AUTHORED VECTOR

    pub fn authored_vec_push(&mut self, value: String) -> app::Result<usize> {
        let index = self.authored_vec.push(LwwRegister::new(value.clone()))?;
        let owner = bs58::encode(env::executor_id()).into_string();
        app::emit!(Event::AuthoredVecPushed {
            index,
            value,
            owner,
        });
        Ok(index)
    }

    pub fn authored_vec_get(&self, index: usize) -> app::Result<Option<String>> {
        Ok(self.authored_vec.get(index)?.map(|r| r.get().clone()))
    }

    pub fn authored_vec_update(&mut self, index: usize, value: String) -> app::Result<()> {
        self.authored_vec
            .update(index, LwwRegister::new(value.clone()))?;
        app::emit!(Event::AuthoredVecUpdated { index, value });
        Ok(())
    }

    pub fn authored_vec_remove(&mut self, index: usize) -> app::Result<()> {
        self.authored_vec.tombstone(index)?;
        app::emit!(Event::AuthoredVecRemoved { index });
        Ok(())
    }

    pub fn authored_vec_get_owner(&self, index: usize) -> app::Result<Option<String>> {
        Ok(self.authored_vec.owner_of(index)?.map(|pk| pk.to_string()))
    }

    pub fn authored_vec_entries(&self) -> app::Result<Vec<String>> {
        Ok(self.authored_vec.iter()?.map(|r| r.get().clone()).collect())
    }

    pub fn authored_vec_len(&self) -> app::Result<usize> {
        self.authored_vec.len().map_err(Into::into)
    }
}
