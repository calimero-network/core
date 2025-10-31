//! Nested CRDT Test Application
//!
//! Tests all nesting patterns for CRDT support:
//! - Map<String, Counter> - counters should sum
//! - Map<String, LwwRegister<T>> - timestamps should win
//! - Map<String, Map<String, String>> - nested maps
//! - Vector<Counter> - element-wise merge
//! - Map<String, Set<String>> - union merge

#![allow(
    unused_crate_dependencies,
    reason = "Dependencies used in build process"
)]

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::{Counter, LwwRegister, UnorderedMap, UnorderedSet, Vector};

#[app::state(emits = TestEvent)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct NestedCrdtTest {
    /// Map of counters - concurrent increments should sum
    pub counters: UnorderedMap<String, Counter>,

    /// Map of LWW registers - latest timestamp wins
    pub registers: UnorderedMap<String, LwwRegister<String>>,

    /// Nested maps - field-level merge
    pub metadata: UnorderedMap<String, UnorderedMap<String, LwwRegister<String>>>,

    /// Vector of counters - element-wise merge
    pub metrics: Vector<Counter>,

    /// Map of sets - union merge
    pub tags: UnorderedMap<String, UnorderedSet<String>>,
}

#[app::event]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub enum TestEvent {
    CounterIncremented {
        key: String,
        value: u64,
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
}

#[app::logic]
impl NestedCrdtTest {
    /// Initialize with empty state
    #[app::init]
    pub fn init() -> NestedCrdtTest {
        NestedCrdtTest {
            counters: UnorderedMap::new(),
            registers: UnorderedMap::new(),
            metadata: UnorderedMap::new(),
            metrics: Vector::new(),
            tags: UnorderedMap::new(),
        }
    }

    // ===== Counter Operations =====

    pub fn increment_counter(&mut self, key: String) -> Result<u64, String> {
        let mut counter = self
            .counters
            .get(&key)
            .map_err(|e| format!("Get failed: {:?}", e))?
            .unwrap_or_else(|| Counter::new());

        counter
            .increment()
            .map_err(|e| format!("Increment failed: {:?}", e))?;

        let value = counter
            .value()
            .map_err(|e| format!("Value failed: {:?}", e))?;

        drop(
            self.counters
                .insert(key.clone(), counter)
                .map_err(|e| format!("Insert failed: {:?}", e))?,
        );

        app::emit!(TestEvent::CounterIncremented { key, value });

        Ok(value)
    }

    pub fn get_counter(&self, key: String) -> Result<u64, String> {
        self.counters
            .get(&key)
            .map_err(|e| format!("Get failed: {:?}", e))?
            .map(|c| c.value().unwrap_or(0))
            .ok_or_else(|| "Counter not found".to_owned())
    }

    // ===== LwwRegister Operations =====

    pub fn set_register(&mut self, key: String, value: String) -> Result<(), String> {
        let register = LwwRegister::new(value.clone());

        drop(
            self.registers
                .insert(key.clone(), register)
                .map_err(|e| format!("Insert failed: {:?}", e))?,
        );

        app::emit!(TestEvent::RegisterSet { key, value });

        Ok(())
    }

    pub fn get_register(&self, key: String) -> Result<String, String> {
        self.registers
            .get(&key)
            .map_err(|e| format!("Get failed: {:?}", e))?
            .map(|r| r.get().clone())
            .ok_or_else(|| "Register not found".to_owned())
    }

    // ===== Nested Map Operations =====

    pub fn set_metadata(
        &mut self,
        outer_key: String,
        inner_key: String,
        value: String,
    ) -> Result<(), String> {
        let mut inner_map = self
            .metadata
            .get(&outer_key)
            .map_err(|e| format!("Get failed: {:?}", e))?
            .unwrap_or_else(|| UnorderedMap::new());

        drop(
            inner_map
                .insert(inner_key.clone(), value.clone().into())
                .map_err(|e| format!("Inner insert failed: {:?}", e))?,
        );

        drop(
            self.metadata
                .insert(outer_key.clone(), inner_map)
                .map_err(|e| format!("Outer insert failed: {:?}", e))?,
        );

        app::emit!(TestEvent::MetadataSet {
            outer_key,
            inner_key,
            value,
        });

        Ok(())
    }

    pub fn get_metadata(&self, outer_key: String, inner_key: String) -> Result<String, String> {
        self.metadata
            .get(&outer_key)
            .map_err(|e| format!("Outer get failed: {:?}", e))?
            .ok_or_else(|| "Outer key not found".to_owned())?
            .get(&inner_key)
            .map_err(|e| format!("Inner get failed: {:?}", e))?
            .ok_or_else(|| "Inner key not found".to_owned())
            .map(|v| v.get().clone())
    }

    // ===== Vector Operations =====

    pub fn push_metric(&mut self, value: u64) -> Result<usize, String> {
        let mut counter = Counter::new();
        for _ in 0..value {
            counter
                .increment()
                .map_err(|e| format!("Increment failed: {:?}", e))?;
        }

        self.metrics
            .push(counter)
            .map_err(|e| format!("Push failed: {:?}", e))?;

        let len = self
            .metrics
            .len()
            .map_err(|e| format!("Len failed: {:?}", e))?;

        app::emit!(TestEvent::MetricPushed { value });

        Ok(len)
    }

    pub fn get_metric(&self, index: usize) -> Result<u64, String> {
        self.metrics
            .get(index)
            .map_err(|e| format!("Get failed: {:?}", e))?
            .ok_or_else(|| "Index out of bounds".to_owned())?
            .value()
            .map_err(|e| format!("Value failed: {:?}", e))
    }

    pub fn metrics_len(&self) -> Result<usize, String> {
        self.metrics
            .len()
            .map_err(|e| format!("Len failed: {:?}", e))
    }

    // ===== Set Operations =====

    pub fn add_tag(&mut self, key: String, tag: String) -> Result<(), String> {
        let mut set = self
            .tags
            .get(&key)
            .map_err(|e| format!("Get failed: {:?}", e))?
            .unwrap_or_else(|| UnorderedSet::new());

        let _ = set
            .insert(tag.clone())
            .map_err(|e| format!("Insert failed: {:?}", e))?;

        drop(
            self.tags
                .insert(key.clone(), set)
                .map_err(|e| format!("Insert failed: {:?}", e))?,
        );

        app::emit!(TestEvent::TagAdded { key, tag });

        Ok(())
    }

    pub fn has_tag(&self, key: String, tag: String) -> Result<bool, String> {
        self.tags
            .get(&key)
            .map_err(|e| format!("Get failed: {:?}", e))?
            .map(|set| set.contains(&tag).unwrap_or(false))
            .ok_or_else(|| "Key not found".to_owned())
    }

    pub fn get_tag_count(&self, key: String) -> Result<u64, String> {
        let count = self
            .tags
            .get(&key)
            .map_err(|e| format!("Get failed: {:?}", e))?
            .ok_or_else(|| "Key not found".to_owned())?
            .iter()
            .map_err(|e| format!("Iter failed: {:?}", e))?
            .count();

        Ok(count as u64)
    }
}
