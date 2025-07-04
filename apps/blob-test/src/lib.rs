#![allow(clippy::len_without_is_empty)]

use std::collections::BTreeMap;

use calimero_sdk::app;
use calimero_sdk::blob::{self, BlobId};
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use thiserror::Error;

#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct BlobTestApp {
    blob_registry: BTreeMap<String, String>, // name -> blob_id_hex mapping
    blob_metadata: BTreeMap<String, BlobMetadata>, // name -> metadata mapping
    blob_count: u32,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
pub struct BlobMetadata {
    pub blob_id: String,
    pub size: u64,
    pub content_type: Option<String>,
    pub uploaded_at: u64, // timestamp
}

#[app::event]
pub enum Event<'a> {
    BlobRegistered { name: &'a str, blob_id: &'a str },
    BlobRead { name: &'a str, blob_id: &'a str },
    BlobUnregistered { name: &'a str },
    TestCompleted { test_name: &'a str, success: bool },
}

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Error<'a> {
    #[error("blob not found: {0}")]
    BlobNotFound(&'a str),
    #[error("blob already exists: {0}")]
    BlobAlreadyExists(&'a str),
    #[error("invalid blob ID: {0}")]
    InvalidBlobId(&'a str),
    #[error("test failed: {0}")]
    TestFailed(&'a str),
}

#[app::logic]
impl BlobTestApp {
    #[app::init]
    pub fn init() -> BlobTestApp {
        app::log!("Initializing BlobTestApp");

        BlobTestApp {
            blob_registry: BTreeMap::new(),
            blob_metadata: BTreeMap::new(),
            blob_count: 0,
        }
    }

    /// Register a blob that was uploaded via REST API
    /// The blob_id should be the base58 string returned from POST /admin-api/blobs/upload
    pub fn register_blob(
        &mut self,
        name: String,
        blob_id: String,
        size: u64,
        content_type: Option<String>,
    ) -> app::Result<()> {
        app::log!("Registering blob: {:?} with ID: {}", name, blob_id);

        // Check if name already exists
        if self.blob_registry.contains_key(&name) {
            app::bail!(Error::BlobAlreadyExists(&name));
        }

        // Parse and validate blob ID (base58 format from REST API)
        let blob_id_obj = blob_id
            .parse::<BlobId>()
            .map_err(|_| Error::InvalidBlobId(&blob_id))?;

        // Check if blob exists in storage
        if !blob::blob_exists(&blob_id_obj) {
            app::bail!(Error::BlobNotFound(&blob_id));
        }

        // Store the mapping and metadata (keep original base58 format)
        self.blob_registry.insert(name.clone(), blob_id.clone());
        self.blob_metadata.insert(
            name.clone(),
            BlobMetadata {
                blob_id: blob_id.clone(),
                size,
                content_type,
                uploaded_at: 0, // TODO: Get actual timestamp
            },
        );
        self.blob_count += 1;

        app::emit!(Event::BlobRegistered {
            name: &name,
            blob_id: &blob_id,
        });

        app::log!(
            "Successfully registered blob '{}' with ID: {}",
            name,
            blob_id
        );
        Ok(())
    }

    /// Diagnostic function to debug blob ID issues step by step
    pub fn debug_blob_id(&self, blob_id: String) -> app::Result<String> {
        app::log!("=== DEBUGGING BLOB ID: {} ===", blob_id);

        // Step 1: Check if we can parse the blob ID
        app::log!("Step 1: Attempting to parse blob ID from base58...");
        let blob_id_obj = match blob_id.parse::<BlobId>() {
            Ok(id) => {
                app::log!("✓ Successfully parsed blob ID");
                id
            }
            Err(e) => {
                app::log!("✗ Failed to parse blob ID: {:?}", e);
                return Ok(format!("PARSING_FAILED: {}", blob_id));
            }
        };

        // Step 2: Check if blob exists using load_blob
        app::log!("Step 2: Checking if blob exists using load_blob...");
        match blob::load_blob(&blob_id_obj) {
            Ok(Some(data)) => {
                app::log!("✓ Blob found! Size: {} bytes", data.len());
                return Ok(format!("SUCCESS: Blob found with {} bytes", data.len()));
            }
            Ok(None) => {
                app::log!("✗ Blob not found in storage");
                return Ok(format!("NOT_FOUND: {}", blob_id));
            }
            Err(e) => {
                app::log!("✗ Error loading blob: {}", e);
                return Ok(format!("LOAD_ERROR: {}", e));
            }
        }
    }

    /// Test the complete workflow with diagnostics
    pub fn test_blob_id_diagnostics(&mut self) -> app::Result<String> {
        app::log!("=== BLOB ID DIAGNOSTICS TEST ===");

        // Step 1: Create a blob using SDK
        app::log!("Step 1: Creating blob with SDK...");
        let test_data = b"Diagnostic test data";
        let sdk_blob_id =
            blob::store_blob(test_data).map_err(|_e| Error::TestFailed("SDK store failed"))?;
        let sdk_blob_id_str = sdk_blob_id.to_string();
        app::log!("SDK created blob: {}", sdk_blob_id_str);

        // Step 2: Check if we can load it back immediately
        app::log!("Step 2: Testing immediate load...");
        let debug_result = self.debug_blob_id(sdk_blob_id_str.clone())?;
        app::log!("Debug result: {}", debug_result);

        // Step 3: Try to register it
        app::log!("Step 3: Attempting registration...");
        match self.register_blob(
            "diagnostic_test".to_string(),
            sdk_blob_id_str.clone(),
            test_data.len() as u64,
            Some("text/plain".to_string()),
        ) {
            Ok(()) => {
                app::log!("✓ Registration successful!");
                Ok("SUCCESS: All steps completed".to_string())
            }
            Err(e) => {
                app::log!("✗ Registration failed: {:?}", e);
                Ok(format!("REGISTRATION_FAILED: {:?}", e))
            }
        }
    }

    /// Compare REST API blob ID format vs SDK blob ID format for same data
    pub fn test_blob_id_format_comparison(&mut self) -> app::Result<String> {
        app::log!("=== BLOB ID FORMAT COMPARISON ===");

        let test_data = b"Format comparison test data";

        // Create blob using SDK
        app::log!("Creating blob with SDK...");
        let sdk_blob_id =
            blob::store_blob(test_data).map_err(|_e| Error::TestFailed("SDK failed"))?;
        let sdk_blob_id_str = sdk_blob_id.to_string();
        app::log!("SDK blob ID: {}", sdk_blob_id_str);
        app::log!("SDK blob ID length: {}", sdk_blob_id_str.len());

        // Test parsing the SDK-generated ID
        app::log!("Testing SDK blob ID parsing...");
        let parsed_sdk_id = sdk_blob_id_str
            .parse::<BlobId>()
            .map_err(|_e| Error::TestFailed("SDK ID parse failed"))?;
        app::log!("✓ SDK blob ID parsed successfully");

        // Test if we can load the SDK blob
        app::log!("Testing SDK blob existence...");
        match blob::load_blob(&parsed_sdk_id) {
            Ok(Some(data)) => {
                app::log!("✓ SDK blob exists and has {} bytes", data.len());
                if data == test_data {
                    app::log!("✓ SDK blob data matches original");
                } else {
                    app::log!("✗ SDK blob data mismatch!");
                }
            }
            Ok(None) => {
                app::log!("✗ SDK blob not found!");
                return Ok("SDK_BLOB_NOT_FOUND".to_string());
            }
            Err(e) => {
                app::log!("✗ SDK blob load error: {}", e);
                return Ok(format!("SDK_BLOB_ERROR: {}", e));
            }
        }

        // Create another blob with same data to see if IDs are deterministic
        app::log!("Creating second blob with same data...");
        let sdk_blob_id2 =
            blob::store_blob(test_data).map_err(|_e| Error::TestFailed("SDK2 failed"))?;
        let sdk_blob_id2_str = sdk_blob_id2.to_string();
        app::log!("SDK blob ID #2: {}", sdk_blob_id2_str);

        if sdk_blob_id_str == sdk_blob_id2_str {
            app::log!("✓ Blob IDs are deterministic (same data = same ID)");
        } else {
            app::log!("! Blob IDs are NOT deterministic (same data = different ID)");
        }

        Ok(format!(
            "SDK_ID: {} (len: {})",
            sdk_blob_id_str,
            sdk_blob_id_str.len()
        ))
    }

    /// Get blob ID for a given name (for use with REST API download)
    pub fn get_blob_id(&self, name: &str) -> app::Result<String> {
        app::log!("Getting blob ID for: {:?}", name);

        let Some(blob_id) = self.blob_registry.get(name) else {
            app::bail!(Error::BlobNotFound(name));
        };

        app::emit!(Event::BlobRead { name, blob_id });

        Ok(blob_id.clone())
    }

    /// Get blob metadata for a given name
    pub fn get_blob_metadata(&self, name: &str) -> app::Result<BlobMetadata> {
        app::log!("Getting blob metadata for: {:?}", name);

        let Some(metadata) = self.blob_metadata.get(name) else {
            app::bail!(Error::BlobNotFound(name));
        };

        Ok(metadata.clone())
    }

    /// List all registered blobs with their metadata
    pub fn list_blobs(&self) -> app::Result<BTreeMap<String, BlobMetadata>> {
        app::log!("Listing {} blobs", self.blob_metadata.len());
        Ok(self.blob_metadata.clone())
    }

    /// Unregister a blob (doesn't delete the actual blob from storage)
    pub fn unregister_blob(&mut self, name: &str) -> app::Result<()> {
        app::log!("Unregistering blob: {:?}", name);

        let Some(_blob_id) = self.blob_registry.remove(name) else {
            app::bail!(Error::BlobNotFound(name));
        };

        self.blob_metadata.remove(name);
        if self.blob_count > 0 {
            self.blob_count -= 1;
        }

        app::emit!(Event::BlobUnregistered { name });

        Ok(())
    }

    /// Create a new blob with given name and data (for backward compatibility/testing)
    pub fn create_blob(&mut self, name: String, data: Vec<u8>) -> app::Result<String> {
        app::log!("Creating blob: {:?} with {} bytes", name, data.len());

        // Check if name already exists
        if self.blob_registry.contains_key(&name) {
            app::bail!(Error::BlobAlreadyExists(&name));
        }

        // Use the simplified blob API to store data
        let blob_id =
            blob::store_blob(&data).map_err(|_e| Error::TestFailed("Failed to store blob"))?;

        // Convert blob ID to base58 string for storage
        let blob_id_str = blob_id.to_string();
        app::log!(
            "Successfully stored {} bytes for blob '{}' with ID: {}",
            data.len(),
            name,
            blob_id_str
        );

        // Store the mapping and metadata
        self.blob_registry.insert(name.clone(), blob_id_str.clone());
        self.blob_metadata.insert(
            name.clone(),
            BlobMetadata {
                blob_id: blob_id_str.clone(),
                size: data.len() as u64,
                content_type: None,
                uploaded_at: 0,
            },
        );
        self.blob_count += 1;

        app::emit!(Event::BlobRegistered {
            name: &name,
            blob_id: &blob_id_str,
        });

        Ok(blob_id_str)
    }

    /// Read blob data by name (for backward compatibility/testing)
    pub fn read_blob(&self, name: &str) -> app::Result<Vec<u8>> {
        app::log!("Reading blob: {:?}", name);

        let Some(blob_id_str) = self.blob_registry.get(name) else {
            app::log!("Blob name '{}' not found in registry", name);
            app::bail!(Error::BlobNotFound(name));
        };

        app::log!("Reading blob with ID: {}", blob_id_str);

        // Parse base58 string to BlobId
        let blob_id = blob_id_str
            .parse::<BlobId>()
            .map_err(|_| Error::TestFailed("Invalid blob ID format"))?;

        // Use the simplified blob API
        let data_opt =
            blob::load_blob(&blob_id).map_err(|_e| Error::TestFailed("Failed to load blob"))?;

        let Some(data) = data_opt else {
            app::bail!(Error::TestFailed("Blob not found in storage"));
        };

        app::log!("Read {} bytes", data.len());

        app::emit!(Event::BlobRead {
            name,
            blob_id: blob_id_str,
        });

        Ok(data)
    }

    /// Test basic blob operations using REST API workflow
    pub fn test_rest_api_workflow(&mut self) -> app::Result<String> {
        app::log!("Testing REST API workflow");

        // This test assumes a blob was uploaded via REST API
        // In a real scenario, the frontend would:
        // 1. POST blob to /admin-api/blobs/upload -> get blob_id
        // 2. Call this method with the returned blob_id

        let test_data = b"Hello, REST API World!";

        // Simulate the workflow by first creating a blob using the internal API
        let blob_id =
            blob::store_blob(test_data).map_err(|_e| Error::TestFailed("Failed to store blob"))?;
        let blob_id_str = blob_id.to_string();

        // Now register it as if it came from REST API
        self.register_blob(
            "rest_test".to_string(),
            blob_id_str.clone(),
            test_data.len() as u64,
            Some("text/plain".to_string()),
        )?;

        // Verify we can get the blob ID back
        let retrieved_id = self.get_blob_id("rest_test")?;

        if retrieved_id == blob_id_str {
            app::emit!(Event::TestCompleted {
                test_name: "rest_api_workflow",
                success: true,
            });
            Ok(format!(
                "REST API workflow test passed! Blob ID: {}",
                blob_id_str
            ))
        } else {
            app::emit!(Event::TestCompleted {
                test_name: "rest_api_workflow",
                success: false,
            });
            app::bail!(Error::TestFailed(
                "Blob ID mismatch in REST API workflow test"
            ));
        }
    }

    /// Test basic blob operations (backward compatibility)
    pub fn test_basic_operations(&mut self) -> app::Result<String> {
        app::log!("Running basic blob operations test");

        let test_data = b"Hello, Blob World!";
        let blob_name = "test_basic".to_string();

        // Create blob
        self.create_blob(blob_name.clone(), test_data.to_vec())?;

        // Read blob back
        let read_data = self.read_blob(&blob_name)?;

        // Check that the data matches exactly
        if read_data == test_data {
            app::emit!(Event::TestCompleted {
                test_name: "basic_operations",
                success: true,
            });
            Ok("Basic operations test passed - real blob data matches exactly!".to_string())
        } else {
            app::emit!(Event::TestCompleted {
                test_name: "basic_operations",
                success: false,
            });
            app::bail!(Error::TestFailed("Data mismatch in basic operations test"));
        }
    }

    /// Test with multipart data (backward compatibility)
    pub fn test_multipart_blob(&mut self, chunks: Vec<Vec<u8>>) -> app::Result<String> {
        app::log!("Testing multipart blob with {} chunks", chunks.len());

        let blob_name = "multipart_test".to_string();

        // Combine all chunks into one blob
        let mut expected_data = Vec::new();
        for (i, chunk) in chunks.iter().enumerate() {
            app::log!("Processing chunk {} of {} bytes", i + 1, chunk.len());
            expected_data.extend_from_slice(chunk);
        }

        // Create the blob with combined data
        self.create_blob(blob_name.clone(), expected_data.clone())?;

        // Verify by reading back
        let read_data = self.read_blob(&blob_name)?;

        if read_data == expected_data {
            app::emit!(Event::TestCompleted {
                test_name: "multipart_blob",
                success: true,
            });
            Ok(format!(
                "Multipart blob test passed - {} chunks, {} total bytes, real data matches exactly!",
                chunks.len(),
                expected_data.len()
            ))
        } else {
            app::emit!(Event::TestCompleted {
                test_name: "multipart_blob",
                success: false,
            });
            app::bail!(Error::TestFailed("Multipart data mismatch"));
        }
    }

    /// Get stats
    pub fn get_stats(&self) -> app::Result<BTreeMap<String, u32>> {
        let mut stats = BTreeMap::new();
        stats.insert("blob_count".to_string(), self.blob_count);
        stats.insert("registry_size".to_string(), self.blob_registry.len() as u32);
        Ok(stats)
    }

    // ========================================
    // COMPREHENSIVE TEST SUITE
    // ========================================

    /// Run all tests in sequence and return detailed results
    pub fn run_all_tests(&mut self) -> app::Result<BTreeMap<String, String>> {
        app::log!("🧪 Starting comprehensive test suite");
        let mut results = BTreeMap::new();

        // Test 1: Basic CRUD operations
        match self.test_basic_crud() {
            Ok(msg) => results.insert("basic_crud".to_string(), format!("✅ {}", msg)),
            Err(e) => results.insert("basic_crud".to_string(), format!("❌ {:?}", e)),
        };

        // Test 2: Error handling
        match self.test_error_conditions() {
            Ok(msg) => results.insert("error_conditions".to_string(), format!("✅ {}", msg)),
            Err(e) => results.insert("error_conditions".to_string(), format!("❌ {:?}", e)),
        };

        // Test 3: REST API workflow simulation
        match self.test_rest_api_workflow_comprehensive() {
            Ok(msg) => results.insert("rest_api_workflow".to_string(), format!("✅ {}", msg)),
            Err(e) => results.insert("rest_api_workflow".to_string(), format!("❌ {:?}", e)),
        };

        // Test 4: Large blob handling
        match self.test_large_blob_handling() {
            Ok(msg) => results.insert("large_blob_handling".to_string(), format!("✅ {}", msg)),
            Err(e) => results.insert("large_blob_handling".to_string(), format!("❌ {:?}", e)),
        };

        // Test 5: Blob ID format validation
        match self.test_blob_id_validation() {
            Ok(msg) => results.insert("blob_id_validation".to_string(), format!("✅ {}", msg)),
            Err(e) => results.insert("blob_id_validation".to_string(), format!("❌ {:?}", e)),
        };

        // Test 6: Metadata handling
        match self.test_metadata_operations() {
            Ok(msg) => results.insert("metadata_operations".to_string(), format!("✅ {}", msg)),
            Err(e) => results.insert("metadata_operations".to_string(), format!("❌ {:?}", e)),
        };

        // Test 7: Concurrent operations simulation
        match self.test_concurrent_operations() {
            Ok(msg) => results.insert("concurrent_operations".to_string(), format!("✅ {}", msg)),
            Err(e) => results.insert("concurrent_operations".to_string(), format!("❌ {:?}", e)),
        };

        // Test 8: Blob ID diagnostics
        match self.test_blob_id_diagnostics() {
            Ok(msg) => results.insert("blob_id_diagnostics".to_string(), format!("✅ {}", msg)),
            Err(e) => results.insert("blob_id_diagnostics".to_string(), format!("❌ {:?}", e)),
        };

        // Test 9: Blob ID format comparison
        match self.test_blob_id_format_comparison() {
            Ok(msg) => results.insert(
                "blob_id_format_comparison".to_string(),
                format!("✅ {}", msg),
            ),
            Err(e) => results.insert(
                "blob_id_format_comparison".to_string(),
                format!("❌ {:?}", e),
            ),
        };

        let passed = results.values().filter(|r| r.starts_with("✅")).count();
        let total = results.len();

        app::log!("🧪 Test suite completed: {}/{} tests passed", passed, total);

        results.insert(
            "summary".to_string(),
            format!("Passed: {}/{} tests", passed, total),
        );
        Ok(results)
    }

    /// Test basic CRUD operations
    fn test_basic_crud(&mut self) -> app::Result<String> {
        app::log!("Testing basic CRUD operations");

        let test_data = b"Hello, CRUD World!";
        let blob_name = "crud_test".to_string();

        // Create
        let blob_id = self.create_blob(blob_name.clone(), test_data.to_vec())?;

        // Read
        let read_data = self.read_blob(&blob_name)?;
        if read_data != test_data {
            app::bail!(Error::TestFailed("Read data doesn't match original"));
        }

        // Get metadata
        let metadata = self.get_blob_metadata(&blob_name)?;
        if metadata.size != test_data.len() as u64 {
            app::bail!(Error::TestFailed("Metadata size mismatch"));
        }

        // List (should contain our blob)
        let blob_list = self.list_blobs()?;
        if !blob_list.contains_key(&blob_name) {
            app::bail!(Error::TestFailed("Blob not found in list"));
        }

        // Delete
        self.unregister_blob(&blob_name)?;

        // Verify deletion
        let blob_list_after = self.list_blobs()?;
        if blob_list_after.contains_key(&blob_name) {
            app::bail!(Error::TestFailed("Blob still exists after deletion"));
        }

        Ok(format!("CRUD operations successful. Blob ID: {}", blob_id))
    }

    /// Test error conditions
    fn test_error_conditions(&mut self) -> app::Result<String> {
        app::log!("Testing error conditions");

        // Test 1: Reading non-existent blob
        match self.read_blob("nonexistent") {
            Err(_) => {} // Expected
            Ok(_) => app::bail!(Error::TestFailed(
                "Should have failed reading non-existent blob"
            )),
        }

        // Test 2: Duplicate registration
        let test_data = b"Duplicate test";
        let blob_name = "duplicate_test".to_string();

        self.create_blob(blob_name.clone(), test_data.to_vec())?;

        match self.create_blob(blob_name.clone(), test_data.to_vec()) {
            Err(_) => {} // Expected
            Ok(_) => app::bail!(Error::TestFailed(
                "Should have failed creating duplicate blob"
            )),
        }

        // Test 3: Invalid blob ID registration
        match self.register_blob(
            "invalid_test".to_string(),
            "invalid_blob_id".to_string(),
            100,
            None,
        ) {
            Err(_) => {} // Expected
            Ok(_) => app::bail!(Error::TestFailed("Should have failed with invalid blob ID")),
        }

        // Test 4: Getting metadata for non-existent blob
        match self.get_blob_metadata("nonexistent") {
            Err(_) => {} // Expected
            Ok(_) => app::bail!(Error::TestFailed(
                "Should have failed getting metadata for non-existent blob"
            )),
        }

        // Cleanup
        self.unregister_blob(&blob_name)?;

        Ok("All error conditions handled correctly".to_string())
    }

    /// Comprehensive REST API workflow test
    fn test_rest_api_workflow_comprehensive(&mut self) -> app::Result<String> {
        app::log!("Testing comprehensive REST API workflow");

        // Simulate multiple file types
        let test_cases = vec![
            ("text_file", b"This is a text file".to_vec(), "text/plain"),
            (
                "json_file",
                br#"{"key": "value", "number": 42}"#.to_vec(),
                "application/json",
            ),
            (
                "binary_file",
                vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
                "image/png",
            ), // PNG header
        ];

        for (name, data, content_type) in test_cases {
            // Step 1: Store blob (simulating REST API upload)
            let blob_id =
                blob::store_blob(&data).map_err(|_| Error::TestFailed("Failed to store blob"))?;
            let blob_id_str = blob_id.to_string();

            // Step 2: Register via JSON RPC
            self.register_blob(
                name.to_string(),
                blob_id_str.clone(),
                data.len() as u64,
                Some(content_type.to_string()),
            )?;

            // Step 3: Verify registration
            let retrieved_id = self.get_blob_id(name)?;
            if retrieved_id != blob_id_str {
                app::bail!(Error::TestFailed(&format!("Blob ID mismatch for {}", name)));
            }

            // Step 4: Verify metadata
            let metadata = self.get_blob_metadata(name)?;
            if metadata.content_type.as_ref() != Some(&content_type.to_string()) {
                app::bail!(Error::TestFailed(&format!(
                    "Content type mismatch for {}",
                    name
                )));
            }

            // Step 5: Verify data integrity
            let read_data = self.read_blob(name)?;
            if read_data != data {
                app::bail!(Error::TestFailed(&format!(
                    "Data integrity check failed for {}",
                    name
                )));
            }
        }

        Ok("REST API workflow test passed for all file types".to_string())
    }

    /// Test large blob handling
    fn test_large_blob_handling(&mut self) -> app::Result<String> {
        app::log!("Testing large blob handling");

        // Create a large test blob (1MB)
        let large_data: Vec<u8> = (0..1024 * 1024).map(|i| (i % 256) as u8).collect();
        let blob_name = "large_blob_test".to_string();

        // Store and register
        let blob_id = self.create_blob(blob_name.clone(), large_data.clone())?;

        // Verify size
        let metadata = self.get_blob_metadata(&blob_name)?;
        if metadata.size != large_data.len() as u64 {
            app::bail!(Error::TestFailed("Large blob size mismatch"));
        }

        // Verify data integrity (sample check - first and last 100 bytes)
        let read_data = self.read_blob(&blob_name)?;
        if read_data.len() != large_data.len() {
            app::bail!(Error::TestFailed("Large blob length mismatch"));
        }

        if &read_data[0..100] != &large_data[0..100]
            || &read_data[large_data.len() - 100..] != &large_data[large_data.len() - 100..]
        {
            app::bail!(Error::TestFailed("Large blob data integrity check failed"));
        }

        // Cleanup
        self.unregister_blob(&blob_name)?;

        Ok(format!(
            "Large blob test passed. Size: {} bytes, ID: {}",
            large_data.len(),
            blob_id
        ))
    }

    /// Test blob ID validation
    fn test_blob_id_validation(&mut self) -> app::Result<String> {
        app::log!("Testing blob ID validation");

        // Test valid blob ID from actual storage
        let test_data = b"Validation test";
        let blob_id = blob::store_blob(test_data)
            .map_err(|_| Error::TestFailed("Failed to store validation test blob"))?;
        let blob_id_str = blob_id.to_string();

        // Should succeed with valid blob ID
        self.register_blob(
            "valid_id_test".to_string(),
            blob_id_str.clone(),
            test_data.len() as u64,
            None,
        )?;

        // Test invalid blob ID formats
        let long_id = "1".repeat(100);
        let invalid_ids = vec![
            "not_base58_!@#$",
            "too_short",
            "",       // empty
            &long_id, // too long
        ];

        for invalid_id in invalid_ids {
            match self.register_blob(
                format!("invalid_{}", invalid_id.len()),
                invalid_id.to_string(),
                100,
                None,
            ) {
                Err(_) => {} // Expected
                Ok(_) => app::bail!(Error::TestFailed(&format!(
                    "Should have rejected invalid ID: {}",
                    invalid_id
                ))),
            }
        }

        // Cleanup
        self.unregister_blob("valid_id_test")?;

        Ok("Blob ID validation test passed".to_string())
    }

    /// Test metadata operations
    fn test_metadata_operations(&mut self) -> app::Result<String> {
        app::log!("Testing metadata operations");

        let test_data = b"Metadata test content";
        let blob_name = "metadata_test".to_string();

        // Create blob with metadata
        let blob_id = self.create_blob(blob_name.clone(), test_data.to_vec())?;

        // Verify initial metadata
        let metadata = self.get_blob_metadata(&blob_name)?;
        if metadata.blob_id != blob_id {
            app::bail!(Error::TestFailed("Metadata blob ID mismatch"));
        }
        if metadata.size != test_data.len() as u64 {
            app::bail!(Error::TestFailed("Metadata size mismatch"));
        }

        // Test metadata in list
        let blob_list = self.list_blobs()?;
        let list_metadata = blob_list
            .get(&blob_name)
            .ok_or_else(|| Error::TestFailed("Blob not found in list"))?;

        if list_metadata.blob_id != metadata.blob_id {
            app::bail!(Error::TestFailed("List metadata mismatch"));
        }

        // Cleanup
        self.unregister_blob(&blob_name)?;

        Ok("Metadata operations test passed".to_string())
    }

    /// Test concurrent operations simulation
    fn test_concurrent_operations(&mut self) -> app::Result<String> {
        app::log!("Testing concurrent operations simulation");

        // Simulate multiple concurrent blob registrations
        let blob_count = 5;
        let mut blob_ids = Vec::new();

        for i in 0..blob_count {
            let test_data = format!("Concurrent test blob {}", i).into_bytes();
            let blob_name = format!("concurrent_test_{}", i);

            let blob_id = self.create_blob(blob_name, test_data)?;
            blob_ids.push(blob_id);
        }

        // Verify all blobs exist
        let blob_list = self.list_blobs()?;
        for i in 0..blob_count {
            let blob_name = format!("concurrent_test_{}", i);
            if !blob_list.contains_key(&blob_name) {
                app::bail!(Error::TestFailed(&format!(
                    "Concurrent blob {} not found",
                    i
                )));
            }
        }

        // Verify stats
        let stats = self.get_stats()?;
        let current_count = stats.get("blob_count").unwrap_or(&0);
        if *current_count < blob_count {
            app::bail!(Error::TestFailed("Blob count inconsistency"));
        }

        // Cleanup all concurrent test blobs
        for i in 0..blob_count {
            let blob_name = format!("concurrent_test_{}", i);
            self.unregister_blob(&blob_name)?;
        }

        Ok(format!(
            "Concurrent operations test passed. Created and cleaned up {} blobs",
            blob_count
        ))
    }

    /// Performance benchmark test
    pub fn test_performance_benchmark(&mut self, iterations: u32) -> app::Result<String> {
        app::log!(
            "Running performance benchmark with {} iterations",
            iterations
        );

        let test_data = b"Performance test data";
        let mut operations = 0;

        for i in 0..iterations {
            let blob_name = format!("perf_test_{}", i);

            // Create
            self.create_blob(blob_name.clone(), test_data.to_vec())?;
            operations += 1;

            // Read
            let _data = self.read_blob(&blob_name)?;
            operations += 1;

            // Get metadata
            let _metadata = self.get_blob_metadata(&blob_name)?;
            operations += 1;

            // Delete
            self.unregister_blob(&blob_name)?;
            operations += 1;
        }

        Ok(format!(
            "Performance benchmark completed: {} operations across {} iterations",
            operations, iterations
        ))
    }
}
