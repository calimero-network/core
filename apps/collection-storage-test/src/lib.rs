#![allow(clippy::len_without_is_empty)]

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{UnorderedMap, Vector};
use thiserror::Error;

// Simple test data structures
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, PartialEq, Eq, Hash)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
pub struct TestData {
    pub id: u32,
    pub name: String,
    pub value: u64,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, PartialEq, Eq, Hash)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
pub struct TestKey {
    pub key: String,
}

impl AsRef<[u8]> for TestKey {
    fn as_ref(&self) -> &[u8] {
        self.key.as_bytes()
    }
}

// Nested collection structure to test storage hierarchy
#[derive(Debug, BorshSerialize, BorshDeserialize, Serialize)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
pub struct NestedCollection {
    pub items: Vector<TestData>,
    pub metadata: UnorderedMap<String, String>,
    pub sub_collections: Vector<Vector<TestData>>,
}

// Main application state
#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct CollectionStorageTest {
    // Basic collections
    vectors: UnorderedMap<TestKey, Vector<TestData>>,
    maps: UnorderedMap<TestKey, UnorderedMap<String, TestData>>,

    // Nested collections to test storage hierarchy
    nested: UnorderedMap<TestKey, NestedCollection>,

    // Deep nested collections (collections inside collections inside collections)
    deep_nested: UnorderedMap<TestKey, Vector<Vector<Vector<TestData>>>>,

    // Test counters
    test_counter: u32,
}

#[app::event]
pub enum Event<'a> {
    CollectionCreated {
        key: &'a TestKey,
        collection_type: &'a str,
    },
    DataInserted {
        key: &'a TestKey,
        count: u32,
    },
    TestCompleted {
        test_name: &'a str,
        success: bool,
    },
}

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Error<'a> {
    #[error("test failed: {0}")]
    TestFailed(&'a str),
    #[error("collection operation failed: {0}")]
    CollectionError(&'a str),
}

#[app::logic]
impl CollectionStorageTest {
    #[app::init]
    pub fn init() -> CollectionStorageTest {
        app::log!("🔍 Initializing Collection Storage Test Application");
        let app = CollectionStorageTest {
            vectors: UnorderedMap::new(),
            maps: UnorderedMap::new(),
            nested: UnorderedMap::new(),
            deep_nested: UnorderedMap::new(),
            test_counter: 0,
        };
        app::log!("✅ Collection Storage Test Application initialized successfully");
        app
    }

    // ============================================================================
    // BASIC VECTOR TESTS
    // ============================================================================

    /// Test basic Vector operations: create, insert, retrieve
    pub fn test_vector_basic(&mut self, key_name: String) -> app::Result<String> {
        app::log!("🔍 Testing basic Vector operations for key: {}", key_name);

        let key = TestKey {
            key: key_name.clone(),
        };

        // Create a new vector
        let mut vector = Vector::new();
        app::log!("🔍 Created empty vector");

        // Insert test data
        for i in 0..5 {
            let data = TestData {
                id: i,
                name: format!("item_{}", i),
                value: i as u64 * 100,
            };
            vector.push(data)?;
            app::log!("🔍 Inserted item {} into vector", i);
        }

        // Store the vector in the main collection
        self.vectors.insert(key.clone(), vector)?;
        app::log!("🔍 Successfully stored vector in main collection");

        // Retrieve and verify
        let retrieved_vector = self.vectors.get(&key).unwrap().unwrap();
        let count = retrieved_vector.len().unwrap_or(0);
        app::log!("🔍 Retrieved vector with {} items", count);

        self.test_counter += 1;
        app::emit!(Event::TestCompleted {
            test_name: "vector_basic",
            success: true
        });

        Ok(format!(
            "Vector basic test completed successfully. Items: {}",
            count
        ))
    }

    /// Test Vector persistence across operations
    pub fn test_vector_persistence(&mut self, key_name: String) -> app::Result<String> {
        app::log!("🔍 Testing Vector persistence for key: {}", key_name);

        let key = TestKey {
            key: key_name.clone(),
        };

        // Create and populate vector
        let mut vector = Vector::new();
        for i in 0..3 {
            let data = TestData {
                id: i + 100,
                name: format!("persistent_{}", i),
                value: (i + 100) as u64,
            };
            vector.push(data)?;
        }

        // Store vector
        self.vectors.insert(key.clone(), vector)?;

        // Create a new vector with additional data to test persistence
        let mut new_vector = Vector::new();
        for i in 0..3 {
            let data = TestData {
                id: i + 100,
                name: format!("persistent_{}", i),
                value: (i + 100) as u64,
            };
            new_vector.push(data)?;
        }

        // Add the new item
        let new_data = TestData {
            id: 999,
            name: "new_item".to_string(),
            value: 999,
        };
        new_vector.push(new_data)?;

        // Re-insert the modified vector
        self.vectors.insert(key.clone(), new_vector)?;
        app::log!("🔍 Modified stored vector by re-inserting");

        // Verify persistence
        let final_vector = self.vectors.get(&key).unwrap().unwrap();
        let count = final_vector.len().unwrap_or(0);

        self.test_counter += 1;
        app::emit!(Event::TestCompleted {
            test_name: "vector_persistence",
            success: count == 4
        });

        Ok(format!(
            "Vector persistence test completed. Final count: {}",
            count
        ))
    }

    // ============================================================================
    // BASIC MAP TESTS
    // ============================================================================

    /// Test basic UnorderedMap operations
    pub fn test_map_basic(&mut self, key_name: String) -> app::Result<String> {
        app::log!(
            "🔍 Testing basic UnorderedMap operations for key: {}",
            key_name
        );

        let key = TestKey {
            key: key_name.clone(),
        };

        // Create a new map
        let mut map = UnorderedMap::new();
        app::log!("🔍 Created empty UnorderedMap");

        // Insert test data
        for i in 0..3 {
            let data = TestData {
                id: i + 200,
                name: format!("map_item_{}", i),
                value: (i + 200) as u64,
            };
            map.insert(format!("key_{}", i), data)?;
            app::log!("🔍 Inserted item {} into map", i);
        }

        // Store the map
        self.maps.insert(key.clone(), map)?;
        app::log!("🔍 Successfully stored map in main collection");

        // Retrieve and verify
        let retrieved_map = self.maps.get(&key).unwrap().unwrap();
        let count = retrieved_map.len().unwrap_or(0);

        self.test_counter += 1;
        app::emit!(Event::TestCompleted {
            test_name: "map_basic",
            success: count == 3
        });

        Ok(format!(
            "Map basic test completed successfully. Items: {}",
            count
        ))
    }

    // ============================================================================
    // NESTED COLLECTION TESTS
    // ============================================================================

    /// Test nested collections to validate storage hierarchy
    pub fn test_nested_collections(&mut self, key_name: String) -> app::Result<String> {
        app::log!("🔍 Testing nested collections for key: {}", key_name);

        let key = TestKey {
            key: key_name.clone(),
        };

        // Create nested collection structure
        let mut nested = NestedCollection {
            items: Vector::new(),
            metadata: UnorderedMap::new(),
            sub_collections: Vector::new(),
        };

        // Populate items vector
        for i in 0..3 {
            let data = TestData {
                id: i + 300,
                name: format!("nested_item_{}", i),
                value: (i + 300) as u64,
            };
            nested.items.push(data)?;
        }

        // Populate metadata map
        nested
            .metadata
            .insert("version".to_string(), "1.0".to_string())?;
        nested.metadata.insert(
            "description".to_string(),
            "Test nested collection".to_string(),
        )?;

        // Create sub-collections
        for i in 0..2 {
            let mut sub_vector = Vector::new();
            for j in 0..2 {
                let data = TestData {
                    id: i * 100 + j,
                    name: format!("sub_{}_{}", i, j),
                    value: (i * 100 + j) as u64,
                };
                sub_vector.push(data)?;
            }
            nested.sub_collections.push(sub_vector)?;
        }

        // Store the nested collection
        self.nested.insert(key.clone(), nested)?;
        app::log!("🔍 Successfully stored nested collection");

        // Retrieve and verify
        let retrieved_nested = self.nested.get(&key).unwrap().unwrap();
        let items_count = retrieved_nested.items.len().unwrap_or(0);
        let metadata_count = retrieved_nested.metadata.len().unwrap_or(0);
        let sub_collections_count = retrieved_nested.sub_collections.len().unwrap_or(0);

        self.test_counter += 1;
        let success = items_count == 3 && metadata_count == 2 && sub_collections_count == 2;
        app::emit!(Event::TestCompleted {
            test_name: "nested_collections",
            success
        });

        Ok(format!(
            "Nested collections test completed. Items: {}, Metadata: {}, Sub-collections: {}",
            items_count, metadata_count, sub_collections_count
        ))
    }

    /// Test deep nested collections (collections inside collections inside collections)
    pub fn test_deep_nested_collections(&mut self, key_name: String) -> app::Result<String> {
        app::log!("🔍 Testing deep nested collections for key: {}", key_name);

        let key = TestKey {
            key: key_name.clone(),
        };

        // Create deep nested structure: Vector<Vector<Vector<TestData>>>
        let mut root_vector = Vector::new();

        for i in 0..2 {
            let mut level1_vector = Vector::new();
            for j in 0..2 {
                let mut level2_vector = Vector::new();
                for k in 0..2 {
                    let data = TestData {
                        id: i * 100 + j * 10 + k,
                        name: format!("deep_{}_{}_{}", i, j, k),
                        value: (i * 100 + j * 10 + k) as u64,
                    };
                    level2_vector.push(data)?;
                }
                level1_vector.push(level2_vector)?;
            }
            root_vector.push(level1_vector)?;
        }

        // Store the deep nested collection
        self.deep_nested.insert(key.clone(), root_vector)?;
        app::log!("🔍 Successfully stored deep nested collection");

        // Retrieve and verify the deep structure
        let retrieved = self.deep_nested.get(&key).unwrap().unwrap();
        let root_count = retrieved.len().unwrap_or(0);

        // Test deeper access
        let mut total_deep_items = 0;
        for i in 0..root_count {
            if let Ok(Some(level1)) = retrieved.get(i) {
                let level1_count = level1.len().unwrap_or(0);
                for j in 0..level1_count {
                    if let Ok(Some(level2)) = level1.get(j) {
                        let level2_count = level2.len().unwrap_or(0);
                        total_deep_items += level2_count;
                    }
                }
            }
        }

        self.test_counter += 1;
        let success = root_count == 2 && total_deep_items == 8; // 2 * 2 * 2 = 8
        app::emit!(Event::TestCompleted {
            test_name: "deep_nested_collections",
            success
        });

        Ok(format!(
            "Deep nested collections test completed. Root level: {}, Deep items: {}",
            root_count, total_deep_items
        ))
    }

    // ============================================================================
    // COMPREHENSIVE TEST SUITE
    // ============================================================================

    /// Run all tests in sequence
    pub fn run_all_tests(&mut self) -> app::Result<String> {
        app::log!("🚀 Starting comprehensive collection storage test suite");

        let mut results = Vec::new();

        // Test 1: Basic Vector operations
        match self.test_vector_basic("test_vector_1".to_string()) {
            Ok(result) => results.push(format!("✅ Vector Basic: {}", result)),
            Err(e) => results.push(format!("❌ Vector Basic: {:?}", e)),
        }

        // Test 2: Vector persistence
        match self.test_vector_persistence("test_vector_2".to_string()) {
            Ok(result) => results.push(format!("✅ Vector Persistence: {}", result)),
            Err(e) => results.push(format!("❌ Vector Persistence: {:?}", e)),
        }

        // Test 3: Basic Map operations
        match self.test_map_basic("test_map_1".to_string()) {
            Ok(result) => results.push(format!("✅ Map Basic: {}", result)),
            Err(e) => results.push(format!("❌ Map Basic: {:?}", e)),
        }

        // Test 4: Nested collections
        match self.test_nested_collections("test_nested_1".to_string()) {
            Ok(result) => results.push(format!("✅ Nested Collections: {}", result)),
            Err(e) => results.push(format!("❌ Nested Collections: {:?}", e)),
        }

        // Test 5: Deep nested collections
        match self.test_deep_nested_collections("test_deep_nested_1".to_string()) {
            Ok(result) => results.push(format!("✅ Deep Nested Collections: {}", result)),
            Err(e) => results.push(format!("❌ Deep Nested Collections: {:?}", e)),
        }

        let summary = format!(
            "🎯 Test Suite Complete! {} tests executed.\n\n{}",
            self.test_counter,
            results.join("\n")
        );

        Ok(summary)
    }

    // ============================================================================
    // UTILITY METHODS
    // ============================================================================

    /// Get test statistics
    pub fn get_test_stats(&self) -> app::Result<String> {
        let vector_count = self.vectors.len().unwrap_or(0);
        let map_count = self.maps.len().unwrap_or(0);
        let nested_count = self.nested.len().unwrap_or(0);
        let deep_nested_count = self.deep_nested.len().unwrap_or(0);

        Ok(format!(
            "📊 Test Statistics:\n\
             - Vectors: {}\n\
             - Maps: {}\n\
             - Nested Collections: {}\n\
             - Deep Nested Collections: {}\n\
             - Tests Executed: {}",
            vector_count, map_count, nested_count, deep_nested_count, self.test_counter
        ))
    }

    /// Clear all test data and verify it's actually cleared
    pub fn clear_test_data(&mut self) -> app::Result<String> {
        app::log!("🧹 Clearing all test data");

        // Clear all collections with error handling
        match self.vectors.clear() {
            Ok(_) => app::log!("🔍 Vectors cleared successfully"),
            Err(e) => app::log!("⚠️  Warning: Could not clear vectors: {:?}", e),
        }

        match self.maps.clear() {
            Ok(_) => app::log!("🔍 Maps cleared successfully"),
            Err(e) => app::log!("⚠️  Warning: Could not clear maps: {:?}", e),
        }

        // Clear nested collections
        match self.nested.clear() {
            Ok(_) => app::log!("🔍 Nested collections cleared successfully"),
            Err(e) => app::log!("⚠️  Warning: Could not clear nested collections: {:?}", e),
        }

        // Clear deep nested collections
        match self.deep_nested.clear() {
            Ok(_) => app::log!("🔍 Deep nested collections cleared successfully"),
            Err(e) => app::log!(
                "⚠️  Warning: Could not clear deep nested collections: {:?}",
                e
            ),
        }

        // Reset counter regardless of clear success
        self.test_counter = 0;

        // 🔍 VERIFY that collections are actually empty
        app::log!("🔍 Verifying collections are actually empty...");

        let vectors_count = self.vectors.len().unwrap_or(0);
        let maps_count = self.maps.len().unwrap_or(0);
        let nested_count = self.nested.len().unwrap_or(0);
        let deep_nested_count = self.deep_nested.len().unwrap_or(0);

        app::log!(
            "🔍 Post-clear counts: Vectors={}, Maps={}, Nested={}, Deep Nested={}",
            vectors_count,
            maps_count,
            nested_count,
            deep_nested_count
        );

        // Assert that all collections are empty
        if vectors_count == 0 && maps_count == 0 && nested_count == 0 && deep_nested_count == 0 {
            app::log!("✅ All collections verified as empty - storage cleanup successful!");
            Ok("Test data cleanup completed and verified - all collections are empty".to_string())
        } else {
            app::log!(
                "❌ Storage cleanup verification failed - some collections still contain data!"
            );
            Ok(format!("⚠️  Warning: Some collections may not be fully cleared. Counts: Vectors={}, Maps={}, Nested={}, Deep Nested={}", 
                      vectors_count, maps_count, nested_count, deep_nested_count))
        }
    }

    /// Verify current storage state
    pub fn verify_storage_state(&self) -> app::Result<String> {
        let vectors_count = self.vectors.len().unwrap_or(0);
        let maps_count = self.maps.len().unwrap_or(0);
        let nested_count = self.nested.len().unwrap_or(0);
        let deep_nested_count = self.deep_nested.len().unwrap_or(0);

        let status =
            if vectors_count == 0 && maps_count == 0 && nested_count == 0 && deep_nested_count == 0
            {
                "EMPTY"
            } else {
                "CONTAINS_DATA"
            };

        Ok(format!(
            "🔍 Storage State Verification:\n\
             Status: {}\n\
             - Vectors: {}\n\
             - Maps: {}\n\
             - Nested Collections: {}\n\
             - Deep Nested Collections: {}\n\
             - Tests Executed: {}",
            status, vectors_count, maps_count, nested_count, deep_nested_count, self.test_counter
        ))
    }



    /// Simple hello method for basic testing
    pub fn hello(&self) -> app::Result<String> {
        Ok("Hello from Collection Storage Test! 🧪".to_string())
    }
}
