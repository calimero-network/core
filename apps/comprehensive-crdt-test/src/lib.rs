//! Comprehensive CRDT Test Application
//!
//! This app tests ALL CRDT types, UserStorage, FrozenStorage, and root-level merging:
//! - Counter
//! - UnorderedMap
//! - Vector
//! - UnorderedSet
//! - RGA (ReplicatedGrowableArray)
//! - LwwRegister
//! - UserStorage (simple and nested)
//! - FrozenStorage
//!
//! The app is designed to test root-level concurrent modifications that trigger merge_root_state.

#![allow(clippy::len_without_is_empty)]

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_sdk::PublicKey;
use calimero_storage::collections::Mergeable;
use calimero_storage::collections::{
    Counter, FrozenStorage, LwwRegister, ReplicatedGrowableArray, UnorderedMap, UnorderedSet,
    UserStorage, Vector,
};
use thiserror::Error;

/// Comprehensive app state with ALL CRDT types and storage types
///
/// This state is designed to test root-level concurrent modifications.
/// Each field can be modified independently, triggering root merge when
/// different nodes modify different fields concurrently.
#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct ComprehensiveCrdtApp {
    // ===== Basic CRDT Types =====
    /// Counter CRDT - concurrent increments should sum
    pub root_counter: Counter,

    /// UnorderedMap - field-level merge
    pub root_map: UnorderedMap<String, LwwRegister<String>>,

    /// Vector - element-wise merge
    pub root_vector: Vector<Counter>,

    /// UnorderedSet - union merge
    pub root_set: UnorderedSet<String>,

    /// RGA - text CRDT for collaborative editing
    pub root_rga: ReplicatedGrowableArray,

    /// LwwRegister - last-write-wins
    pub root_register: LwwRegister<String>,

    // ===== Storage Types =====
    /// UserStorage - simple user-owned data
    pub user_storage_simple: UserStorage<LwwRegister<String>>,

    /// UserStorage - nested user-owned data
    pub user_storage_nested: UserStorage<NestedUserData>,

    /// FrozenStorage - content-addressable immutable data
    pub frozen_storage: FrozenStorage<String>,
}

/// Nested user data structure for testing nested UserStorage
#[derive(Debug, BorshSerialize, BorshDeserialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct NestedUserData {
    pub map: UnorderedMap<String, LwwRegister<String>>,
    pub counter: Counter,
}

impl Mergeable for NestedUserData {
    fn merge(
        &mut self,
        other: &Self,
    ) -> Result<(), calimero_storage::collections::crdt_meta::MergeError> {
        self.map.merge(&other.map)?;
        self.counter.merge(&other.counter)?;
        Ok(())
    }
}

#[app::event]
pub enum Event<'a> {
    CounterIncremented { value: u64 },
    MapEntrySet { key: &'a str, value: &'a str },
    VectorPushed { value: u64 },
    SetItemAdded { item: &'a str },
    RgaTextInserted { position: usize, text: &'a str },
    RegisterSet { value: &'a str },
    UserSimpleSet { executor_id: PublicKey, value: &'a str },
    UserNestedSet {
        executor_id: PublicKey,
        key: &'a str,
        value: &'a str,
    },
    FrozenAdded { hash: [u8; 32], value: &'a str },
}

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Error<'a> {
    #[error("key not found: {0}")]
    NotFound(&'a str),
    #[error("User data not found for key: {0}")]
    UserNotFound(PublicKey),
    #[error("Frozen data not found for hash: {0}")]
    FrozenNotFound(&'a str),
}

#[app::logic]
impl ComprehensiveCrdtApp {
    #[app::init]
    pub fn init() -> ComprehensiveCrdtApp {
        ComprehensiveCrdtApp {
            root_counter: Counter::new(),
            root_map: UnorderedMap::new(),
            root_vector: Vector::new(),
            root_set: UnorderedSet::new(),
            root_rga: ReplicatedGrowableArray::new(),
            root_register: LwwRegister::new(String::new()),
            user_storage_simple: UserStorage::new(),
            user_storage_nested: UserStorage::new(),
            frozen_storage: FrozenStorage::new(),
        }
    }

    // ===== Counter Operations =====

    /// Increment the root counter
    pub fn increment_root_counter(&mut self) -> Result<u64, String> {
        self.root_counter
            .increment()
            .map_err(|e| format!("Increment failed: {:?}", e))?;
        let value = self
            .root_counter
            .value()
            .map_err(|e| format!("Value failed: {:?}", e))?;
        app::emit!(Event::CounterIncremented { value });
        Ok(value)
    }

    /// Get the root counter value
    pub fn get_root_counter(&self) -> Result<u64, String> {
        self.root_counter
            .value()
            .map_err(|e| format!("Value failed: {:?}", e))
    }

    // ===== UnorderedMap Operations =====

    /// Set a value in the root map
    pub fn set_root_map(&mut self, key: String, value: String) -> Result<(), String> {
        self.root_map
            .insert(key.clone(), value.clone().into())
            .map_err(|e| format!("Insert failed: {:?}", e))?;
        app::emit!(Event::MapEntrySet {
            key: &key,
            value: &value
        });
        Ok(())
    }

    /// Get a value from the root map
    pub fn get_root_map(&self, key: &str) -> Result<Option<String>, String> {
        Ok(self
            .root_map
            .get(key)
            .map_err(|e| format!("Get failed: {:?}", e))?
            .map(|r| r.get().clone()))
    }

    // ===== Vector Operations =====

    /// Push a counter to the root vector
    pub fn push_root_vector(&mut self, value: u64) -> Result<usize, String> {
        let mut counter = Counter::new();
        for _ in 0..value {
            counter
                .increment()
                .map_err(|e| format!("Increment failed: {:?}", e))?;
        }
        self.root_vector
            .push(counter)
            .map_err(|e| format!("Push failed: {:?}", e))?;
        let len = self
            .root_vector
            .len()
            .map_err(|e| format!("Len failed: {:?}", e))?;
        app::emit!(Event::VectorPushed { value });
        Ok(len)
    }

    /// Get a counter from the root vector
    pub fn get_root_vector(&self, index: usize) -> Result<Option<u64>, String> {
        Ok(self
            .root_vector
            .get(index)
            .map_err(|e| format!("Get failed: {:?}", e))?
            .map(|c| c.value().unwrap_or(0)))
    }

    /// Get the root vector length
    pub fn get_root_vector_len(&self) -> Result<usize, String> {
        self.root_vector
            .len()
            .map_err(|e| format!("Len failed: {:?}", e))
    }

    // ===== UnorderedSet Operations =====

    /// Add an item to the root set
    pub fn add_root_set(&mut self, item: String) -> Result<(), String> {
        self.root_set
            .insert(item.clone())
            .map_err(|e| format!("Insert failed: {:?}", e))?;
        app::emit!(Event::SetItemAdded { item: &item });
        Ok(())
    }

    /// Check if an item is in the root set
    pub fn has_root_set(&self, item: &str) -> Result<bool, String> {
        self.root_set
            .contains(item)
            .map_err(|e| format!("Contains failed: {:?}", e))
    }

    /// Get the root set size
    pub fn get_root_set_size(&self) -> Result<usize, String> {
        Ok(self
            .root_set
            .iter()
            .map_err(|e| format!("Iter failed: {:?}", e))?
            .count())
    }

    // ===== RGA Operations =====

    /// Insert text into the root RGA
    pub fn insert_root_rga(&mut self, position: usize, text: String) -> Result<(), String> {
        self.root_rga
            .insert_str(position, &text)
            .map_err(|e| format!("Insert failed: {:?}", e))?;
        app::emit!(Event::RgaTextInserted {
            position,
            text: &text
        });
        Ok(())
    }

    /// Get text from the root RGA
    pub fn get_root_rga_text(&self) -> Result<String, String> {
        self.root_rga
            .get_text()
            .map_err(|e| format!("Get text failed: {:?}", e))
    }

    /// Get the root RGA length
    pub fn get_root_rga_len(&self) -> Result<usize, String> {
        self.root_rga
            .len()
            .map_err(|e| format!("Len failed: {:?}", e))
    }

    // ===== LwwRegister Operations =====

    /// Set the root register value
    pub fn set_root_register(&mut self, value: String) -> Result<(), String> {
        self.root_register.set(value.clone());
        app::emit!(Event::RegisterSet { value: &value });
        Ok(())
    }

    /// Get the root register value
    pub fn get_root_register(&self) -> Result<String, String> {
        Ok(self.root_register.get().clone())
    }

    // ===== UserStorage Simple Operations =====

    /// Set a simple value for the current user
    pub fn set_user_simple(&mut self, value: String) -> Result<(), String> {
        let executor_id = calimero_sdk::env::executor_id();
        self.user_storage_simple
            .insert(value.clone().into())
            .map_err(|e| format!("Insert failed: {:?}", e))?;
        app::emit!(Event::UserSimpleSet {
            executor_id: executor_id.into(),
            value: &value
        });
        Ok(())
    }

    /// Get the simple value for the current user
    pub fn get_user_simple(&self) -> Result<Option<String>, String> {
        Ok(self
            .user_storage_simple
            .get()
            .map_err(|e| format!("Get failed: {:?}", e))?
            .map(|v| v.get().clone()))
    }

    /// Get the simple value for a specific user
    pub fn get_user_simple_for(&self, user_key: PublicKey) -> Result<Option<String>, String> {
        Ok(self
            .user_storage_simple
            .get_for_user(&user_key)
            .map_err(|e| format!("Get for user failed: {:?}", e))?
            .map(|v| v.get().clone()))
    }

    // ===== UserStorage Nested Operations =====

    /// Set a nested key-value pair for the current user
    pub fn set_user_nested(&mut self, key: String, value: String) -> Result<(), String> {
        let executor_id = calimero_sdk::env::executor_id();
        let mut nested_data = self
            .user_storage_nested
            .get()
            .map_err(|e| format!("Get failed: {:?}", e))?
            .unwrap_or_default();
        nested_data
            .map
            .insert(key.clone(), value.clone().into())
            .map_err(|e| format!("Map insert failed: {:?}", e))?;
        self.user_storage_nested
            .insert(nested_data)
            .map_err(|e| format!("Insert failed: {:?}", e))?;
        app::emit!(Event::UserNestedSet {
            executor_id: executor_id.into(),
            key: &key,
            value: &value
        });
        Ok(())
    }

    /// Increment the nested counter for the current user
    pub fn increment_user_nested_counter(&mut self) -> Result<u64, String> {
        let mut nested_data = self
            .user_storage_nested
            .get()
            .map_err(|e| format!("Get failed: {:?}", e))?
            .unwrap_or_default();
        nested_data
            .counter
            .increment()
            .map_err(|e| format!("Increment failed: {:?}", e))?;
        let value = nested_data
            .counter
            .value()
            .map_err(|e| format!("Value failed: {:?}", e))?;
        self.user_storage_nested
            .insert(nested_data)
            .map_err(|e| format!("Insert failed: {:?}", e))?;
        Ok(value)
    }

    /// Get a nested value for the current user
    pub fn get_user_nested(&self, key: &str) -> Result<Option<String>, String> {
        let nested_data = self
            .user_storage_nested
            .get()
            .map_err(|e| format!("Get failed: {:?}", e))?;
        match nested_data {
            Some(data) => Ok(data
                .map
                .get(key)
                .map_err(|e| format!("Map get failed: {:?}", e))?
                .map(|v| v.get().clone())),
            None => Ok(None),
        }
    }

    /// Get the nested counter value for the current user
    pub fn get_user_nested_counter(&self) -> Result<u64, String> {
        let nested_data = self
            .user_storage_nested
            .get()
            .map_err(|e| format!("Get failed: {:?}", e))?;
        match nested_data {
            Some(data) => data
                .counter
                .value()
                .map_err(|e| format!("Value failed: {:?}", e)),
            None => Ok(0),
        }
    }

    /// Get a nested value for a specific user
    pub fn get_user_nested_for(&self, user_key: PublicKey, key: &str) -> Result<Option<String>, String> {
        let nested_data = self
            .user_storage_nested
            .get_for_user(&user_key)
            .map_err(|e| format!("Get for user failed: {:?}", e))?;
        match nested_data {
            Some(data) => Ok(data
                .map
                .get(key)
                .map_err(|e| format!("Map get failed: {:?}", e))?
                .map(|v| v.get().clone())),
            None => Ok(None),
        }
    }

    // ===== FrozenStorage Operations =====

    /// Add a value to frozen storage
    pub fn add_frozen(&mut self, value: String) -> Result<String, String> {
        let hash = self
            .frozen_storage
            .insert(value.clone().into())
            .map_err(|e| format!("Insert failed: {:?}", e))?;
        app::emit!(Event::FrozenAdded {
            hash,
            value: &value
        });
        Ok(hex::encode(hash))
    }

    /// Get a value from frozen storage by hash
    pub fn get_frozen(&self, hash_hex: String) -> Result<String, String> {
        let mut hash = [0u8; 32];
        hex::decode_to_slice(hash_hex.clone(), &mut hash[..])
            .map_err(|_| "Invalid hash hex".to_string())?;
        self.frozen_storage
            .get(&hash)
            .map_err(|e| format!("Get failed: {:?}", e))?
            .map(|v| v.clone())
            .ok_or_else(|| format!("Frozen data not found for hash: {}", hash_hex))
    }
}
