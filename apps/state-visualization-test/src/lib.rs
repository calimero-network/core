//! State Visualization Test App
//!
//! This app is designed to test merodb's state visualization and schema inference
//! capabilities. It includes various CRDT collection types to verify that:
//!
//! 1. `field_name` is correctly stored in entity metadata
//! 2. Schema inference can detect all field types from the database
//! 3. The GUI correctly displays different collection types
//!
//! This is NOT meant for production use - it's a test fixture for merodb development.

#![allow(clippy::len_without_is_empty)]

use std::collections::BTreeMap;

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{Counter, LwwRegister, UnorderedMap, UnorderedSet, Vector};

/// Test state with multiple CRDT collection types for visualization testing.
///
/// Each field uses a different CRDT type to verify schema inference:
/// - `items`: UnorderedMap<String, LwwRegister<String>> - key-value pairs
/// - `operation_count`: Counter - grow-only counter
/// - `operation_history`: Vector<LwwRegister<String>> - ordered list of operations
/// - `tags`: UnorderedSet<String> - unique tags
/// - `metadata`: LwwRegister<String> - single value register
#[app::state]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct VisualizationTest {
    /// Key-value pairs stored as UnorderedMap
    items: UnorderedMap<String, LwwRegister<String>>,
    /// Total number of operations performed (Counter)
    operation_count: Counter,
    /// History of operations (Vector)
    /// Note: Uses LwwRegister<String> because Vector<T> requires T: Mergeable
    operation_history: Vector<LwwRegister<String>>,
    /// Tags associated with entries (UnorderedSet)
    tags: UnorderedSet<String>,
    /// Store metadata (LwwRegister)
    metadata: LwwRegister<String>,
}

#[derive(Debug, thiserror::Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Error<'a> {
    #[error("key not found: {0}")]
    NotFound(&'a str),
}

#[app::logic]
impl VisualizationTest {
    // =========================================================================
    // Item Operations (UnorderedMap)
    // =========================================================================

    /// Set a key-value pair
    pub fn set(&mut self, key: String, value: String) -> app::Result<()> {
        app::log!("Setting key: {:?} to value: {:?}", key, value);

        self.items.insert(key.clone(), LwwRegister::new(value.clone()))?;
        self.operation_count.increment()?;
        self.operation_history
            .push(LwwRegister::new(format!("Set: {} = {}", key, value)))?;

        Ok(())
    }

    /// Get a value by key
    pub fn get(&self, key: &str) -> app::Result<Option<String>> {
        Ok(self.items.get(key)?.map(|v| v.get().clone()))
    }

    /// Get all entries
    pub fn entries(&self) -> app::Result<BTreeMap<String, String>> {
        Ok(self
            .items
            .entries()?
            .map(|(k, v)| (k, v.get().clone()))
            .collect())
    }

    /// Remove an entry
    pub fn remove(&mut self, key: &str) -> app::Result<Option<String>> {
        let result = self.items.remove(key)?.map(|v| v.get().clone());
        if result.is_some() {
            self.operation_count.increment()?;
            self.operation_history
                .push(LwwRegister::new(format!("Removed: {}", key)))?;
        }
        Ok(result)
    }

    // =========================================================================
    // Tag Operations (UnorderedSet)
    // =========================================================================

    /// Add a tag
    pub fn add_tag(&mut self, tag: String) -> app::Result<bool> {
        let inserted = self.tags.insert(tag.clone())?;
        if inserted {
            self.operation_history
                .push(LwwRegister::new(format!("Added tag: {}", tag)))?;
        }
        Ok(inserted)
    }

    /// Remove a tag
    pub fn remove_tag(&mut self, tag: &str) -> app::Result<bool> {
        let removed = self.tags.remove(tag)?;
        if removed {
            self.operation_history
                .push(LwwRegister::new(format!("Removed tag: {}", tag)))?;
        }
        Ok(removed)
    }

    /// Get all tags
    pub fn get_tags(&self) -> app::Result<Vec<String>> {
        self.tags.entries().map(|iter| iter.collect())
    }

    // =========================================================================
    // Metadata Operations (LwwRegister)
    // =========================================================================

    /// Set store metadata
    pub fn set_metadata(&mut self, metadata: String) -> app::Result<()> {
        self.metadata.set(metadata);
        Ok(())
    }

    /// Get store metadata
    pub fn get_metadata(&self) -> String {
        self.metadata.get().clone()
    }

    // =========================================================================
    // Counter & History Operations
    // =========================================================================

    /// Get operation count
    pub fn get_operation_count(&self) -> app::Result<u64> {
        self.operation_count.value().map_err(Into::into)
    }

    /// Get operation history
    pub fn get_operation_history(&self) -> app::Result<Vec<String>> {
        let len = self.operation_history.len()?;
        let mut history = Vec::new();
        for i in 0..len {
            if let Some(entry) = self.operation_history.get(i)? {
                history.push(entry.get().clone());
            }
        }
        Ok(history)
    }

    // =========================================================================
    // Test Data Population
    // =========================================================================

    /// Populate the store with sample data for testing visualization.
    /// Creates multiple entries in each collection type.
    pub fn populate_sample_data(&mut self) -> app::Result<()> {
        app::log!("Populating sample data for visualization testing");

        // Set store metadata
        self.metadata
            .set("Visualization Test Store - sample data".to_string());

        // Add sample items (UnorderedMap entries)
        let sample_items = [
            ("user:alice", "Alice Johnson"),
            ("user:bob", "Bob Smith"),
            ("user:charlie", "Charlie Brown"),
            ("config:theme", "dark"),
            ("config:language", "en-US"),
            ("config:timezone", "UTC"),
            ("product:1001", "Laptop Pro"),
            ("product:1002", "Wireless Mouse"),
            ("product:1003", "Mechanical Keyboard"),
            ("product:1004", "4K Monitor"),
            ("session:abc123", "active"),
            ("session:def456", "active"),
            ("cache:homepage", "cached_content_here"),
            ("cache:dashboard", "dashboard_content"),
            ("cache:settings", "settings_content"),
        ];

        for (key, value) in sample_items {
            self.items
                .insert(key.to_string(), LwwRegister::new(value.to_string()))?;
            self.operation_count.increment()?;
            self.operation_history
                .push(LwwRegister::new(format!("Inserted: {} = {}", key, value)))?;
        }

        // Add sample tags (UnorderedSet entries)
        let sample_tags = [
            "important",
            "urgent",
            "archived",
            "featured",
            "pinned",
            "read",
            "unread",
            "starred",
            "draft",
            "published",
        ];

        for tag in sample_tags {
            self.tags.insert(tag.to_string())?;
        }

        // Add more history entries (Vector entries)
        let additional_history = [
            "System initialized",
            "Connected to network",
            "Loaded configuration",
            "User session started",
            "Cache warmed up",
        ];

        for entry in additional_history {
            self.operation_history
                .push(LwwRegister::new(entry.to_string()))?;
        }

        Ok(())
    }

    /// Get statistics about all collections
    pub fn get_stats(&self) -> app::Result<BTreeMap<String, u64>> {
        let mut stats = BTreeMap::new();
        stats.insert("items_count".to_string(), self.items.len()? as u64);
        stats.insert("tags_count".to_string(), self.tags.len()? as u64);
        stats.insert(
            "history_count".to_string(),
            self.operation_history.len()? as u64,
        );
        stats.insert("operation_count".to_string(), self.operation_count.value()?);
        Ok(stats)
    }
}
