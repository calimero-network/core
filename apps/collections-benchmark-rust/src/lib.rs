//! Collections Benchmark App (Rust SDK)
//!
//! Simple operations for throughput testing:
//! - Single insert/get operations (no batch operations)
//! - Throughput calculated on client side
//! - Used with parallel workflow steps for load testing
#![allow(
    unused_crate_dependencies,
    reason = "Dependencies used in build process"
)]
use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::{
    LwwRegister, ReplicatedGrowableArray, UnorderedMap, UnorderedSet, Vector,
};
/// Type alias for Counter with default const generic (no decrement)
pub type Counter = calimero_storage::collections::Counter<false>;
/// Type alias for PNCounter (supports both increment and decrement)
pub type PNCounter = calimero_storage::collections::Counter<true>;
/// Application state for benchmarking
#[app::state]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct CollectionsBenchmark {
    /// Counter for tracking operations
    pub operation_count: Counter,
    // Collections for benchmarking
    pub test_map: UnorderedMap<String, Counter>,
    pub test_nested_map: UnorderedMap<String, UnorderedMap<String, Counter>>,
    pub test_deep_nested_map:
        UnorderedMap<String, UnorderedMap<String, UnorderedMap<String, Counter>>>,
    pub test_vector: Vector<Counter>,
    pub test_set: UnorderedSet<String>,
    pub test_registers: UnorderedMap<String, LwwRegister<String>>,
    pub test_rga: ReplicatedGrowableArray,
}
#[app::logic]
impl CollectionsBenchmark {
    #[app::init]
    pub fn init() -> CollectionsBenchmark {
        CollectionsBenchmark {
            operation_count: Counter::new(),
            test_map: UnorderedMap::new(),
            test_nested_map: UnorderedMap::new(),
            test_deep_nested_map: UnorderedMap::new(),
            test_vector: Vector::new(),
            test_set: UnorderedSet::new(),
            test_registers: UnorderedMap::new(),
            test_rga: ReplicatedGrowableArray::new(),
        }
    }
    /// Insert a single key-value pair into the test map
    /// Returns the operation count after insertion
    pub fn map_insert(&mut self, key: String) -> Result<u64, String> {
        let value = Counter::new();
        self.test_map
            .insert(key, value)
            .map_err(|e| format!("Insert failed: {:?}", e))?;

        // Increment operation count
        self.operation_count
            .increment()
            .map_err(|e| format!("Increment failed: {:?}", e))?;

        self.operation_count
            .value()
            .map_err(|e| format!("Get operation count failed: {:?}", e))
    }
    /// Get a value from the test map by key
    /// Returns the operation count (always 0 for read-only operations)
    pub fn map_get(&self, key: String) -> Result<u64, String> {
        let _ = self
            .test_map
            .get(&key)
            .map_err(|e| format!("Get failed: {:?}", e))?;
        // Note: We can't increment operation_count here because this is a read-only method
        // The client will track get operations separately
        Ok(0)
    }
    /// Get the current operation count
    pub fn get_operation_count(&self) -> Result<u64, String> {
        self.operation_count
            .value()
            .map_err(|e| format!("Get operation count failed: {:?}", e))
    }

    /// Get the current timestamp in nanoseconds (for timing measurements)
    pub fn get_timestamp(&self) -> Result<u64, String> {
        Ok(calimero_sdk::env::time_now())
    }

    // NESTED MAP METHODS (Level 2)

    /// Insert into nested map (outer_key, inner_key)
    pub fn nested_map_insert(
        &mut self,
        outer_key: String,
        inner_key: String,
    ) -> Result<u64, String> {
        // Get or create inner map
        let mut inner_map = self
            .test_nested_map
            .get(&outer_key)
            .map_err(|e| format!("Get outer key failed: {:?}", e))?
            .unwrap_or_else(|| UnorderedMap::new());

        let value = Counter::new();
        inner_map
            .insert(inner_key, value)
            .map_err(|e| format!("Insert into inner map failed: {:?}", e))?;

        self.test_nested_map
            .insert(outer_key, inner_map)
            .map_err(|e| format!("Insert outer key failed: {:?}", e))?;

        self.operation_count
            .increment()
            .map_err(|e| format!("Increment failed: {:?}", e))?;

        self.operation_count
            .value()
            .map_err(|e| format!("Get operation count failed: {:?}", e))
    }

    /// Get from nested map (outer_key, inner_key)
    pub fn nested_map_get(&self, outer_key: String, inner_key: String) -> Result<u64, String> {
        let inner_map = self
            .test_nested_map
            .get(&outer_key)
            .map_err(|e| format!("Get outer key failed: {:?}", e))?
            .ok_or_else(|| "Outer key not found".to_string())?;

        let _ = inner_map
            .get(&inner_key)
            .map_err(|e| format!("Get inner key failed: {:?}", e))?;

        Ok(0)
    }

    // DEEP NESTED MAP METHODS (Level 3)

    /// Insert into deep nested map (key1, key2, key3)
    pub fn deep_nested_map_insert(
        &mut self,
        key1: String,
        key2: String,
        key3: String,
    ) -> Result<u64, String> {
        // Get or create level2 map
        let mut level2 = self
            .test_deep_nested_map
            .get(&key1)
            .map_err(|e| format!("Get key1 failed: {:?}", e))?
            .unwrap_or_else(|| UnorderedMap::new());

        // Get or create level3 map
        let mut level3 = level2
            .get(&key2)
            .map_err(|e| format!("Get key2 failed: {:?}", e))?
            .unwrap_or_else(|| UnorderedMap::new());

        let value = Counter::new();
        level3
            .insert(key3, value)
            .map_err(|e| format!("Insert into level3 failed: {:?}", e))?;

        level2
            .insert(key2, level3)
            .map_err(|e| format!("Insert into level2 failed: {:?}", e))?;

        self.test_deep_nested_map
            .insert(key1, level2)
            .map_err(|e| format!("Insert into level1 failed: {:?}", e))?;

        self.operation_count
            .increment()
            .map_err(|e| format!("Increment failed: {:?}", e))?;

        self.operation_count
            .value()
            .map_err(|e| format!("Get operation count failed: {:?}", e))
    }

    /// Get from deep nested map (key1, key2, key3)
    pub fn deep_nested_map_get(
        &self,
        key1: String,
        key2: String,
        key3: String,
    ) -> Result<u64, String> {
        let level2 = self
            .test_deep_nested_map
            .get(&key1)
            .map_err(|e| format!("Get key1 failed: {:?}", e))?
            .ok_or_else(|| "Key1 not found".to_string())?;

        let level3 = level2
            .get(&key2)
            .map_err(|e| format!("Get key2 failed: {:?}", e))?
            .ok_or_else(|| "Key2 not found".to_string())?;

        let _ = level3
            .get(&key3)
            .map_err(|e| format!("Get key3 failed: {:?}", e))?;

        Ok(0)
    }

    // VECTOR METHODS

    /// Push a value to the vector
    pub fn vector_push(&mut self) -> Result<u64, String> {
        let value = Counter::new();
        self.test_vector
            .push(value)
            .map_err(|e| format!("Push failed: {:?}", e))?;

        self.operation_count
            .increment()
            .map_err(|e| format!("Increment failed: {:?}", e))?;

        self.operation_count
            .value()
            .map_err(|e| format!("Get operation count failed: {:?}", e))
    }

    /// Get from vector by index
    pub fn vector_get(&self, index: u32) -> Result<u64, String> {
        let _ = self
            .test_vector
            .get(index as usize)
            .map_err(|e| format!("Get failed: {:?}", e))?;

        Ok(0)
    }

    // SET METHODS

    /// Insert into set
    pub fn set_insert(&mut self, value: String) -> Result<u64, String> {
        self.test_set
            .insert(value)
            .map_err(|e| format!("Insert failed: {:?}", e))?;

        self.operation_count
            .increment()
            .map_err(|e| format!("Increment failed: {:?}", e))?;

        self.operation_count
            .value()
            .map_err(|e| format!("Get operation count failed: {:?}", e))
    }

    /// Check if set contains value
    pub fn set_contains(&self, value: String) -> Result<u64, String> {
        let _ = self
            .test_set
            .contains(&value)
            .map_err(|e| format!("Contains failed: {:?}", e))?;

        Ok(0)
    }

    // REGISTER METHODS

    /// Set a register value
    pub fn register_set(&mut self, key: String, value: String) -> Result<u64, String> {
        let register = LwwRegister::new(value);
        self.test_registers
            .insert(key, register)
            .map_err(|e| format!("Insert failed: {:?}", e))?;

        self.operation_count
            .increment()
            .map_err(|e| format!("Increment failed: {:?}", e))?;

        self.operation_count
            .value()
            .map_err(|e| format!("Get operation count failed: {:?}", e))
    }

    /// Get a register value
    pub fn register_get(&self, key: String) -> Result<u64, String> {
        let register = self
            .test_registers
            .get(&key)
            .map_err(|e| format!("Get failed: {:?}", e))?
            .ok_or_else(|| "Key not found".to_string())?;

        let _ = register.get();

        Ok(0)
    }

    // RGA (REPLICATED GROWABLE ARRAY) METHODS

    /// Insert a string into RGA at index
    pub fn rga_insert(&mut self, index: u32, text: String) -> Result<u64, String> {
        self.test_rga
            .insert_str(index as usize, &text)
            .map_err(|e| format!("Insert failed: {:?}", e))?;

        self.operation_count
            .increment()
            .map_err(|e| format!("Increment failed: {:?}", e))?;

        self.operation_count
            .value()
            .map_err(|e| format!("Get operation count failed: {:?}", e))
    }

    /// Get text from RGA
    pub fn rga_get_text(&self) -> Result<u64, String> {
        let _ = self
            .test_rga
            .get_text()
            .map_err(|e| format!("Get text failed: {:?}", e))?;

        Ok(0)
    }
}
