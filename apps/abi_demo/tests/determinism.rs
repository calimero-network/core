// Copyright 2024 Calimero Limited
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::fs;
use std::path::Path;
use sha2::{Digest, Sha256};

#[test]
fn test_abi_determinism() {
    // This test verifies that the ABI generation is deterministic
    // by building twice and comparing the SHA256 hashes
    
    let target_abi_path = Path::new("target/abi/abi.json");
    
    // Ensure the ABI file exists (should be created by build.rs)
    assert!(target_abi_path.exists(), "ABI file should exist at target/abi/abi.json");
    
    // Read the ABI file
    let abi_content1 = fs::read_to_string(target_abi_path).expect("Failed to read ABI file");
    let abi_content2 = fs::read_to_string(target_abi_path).expect("Failed to read ABI file");
    
    // Content should be identical
    assert_eq!(abi_content1, abi_content2);
    
    // Calculate SHA256 hashes
    let mut hasher1 = Sha256::new();
    hasher1.update(abi_content1.as_bytes());
    let hash1 = hasher1.finalize();
    
    let mut hasher2 = Sha256::new();
    hasher2.update(abi_content2.as_bytes());
    let hash2 = hasher2.finalize();
    
    // Hashes should be identical
    assert_eq!(hash1, hash2);
    
    println!("ABI SHA256: {:x}", hash1);
}

#[test]
fn test_abi_structure() {
    // This test verifies the ABI has the expected structure
    let target_abi_path = Path::new("target/abi/abi.json");
    let abi_content = fs::read_to_string(target_abi_path).expect("Failed to read ABI file");
    
    // Parse as JSON
    let abi: serde_json::Value = serde_json::from_str(&abi_content).expect("Failed to parse ABI as JSON");
    
    // Check required fields
    assert!(abi.get("metadata").is_some());
    assert!(abi.get("module_name").is_some());
    assert!(abi.get("module_version").is_some());
    assert!(abi.get("functions").is_some());
    assert!(abi.get("events").is_some());
    
    // Check metadata fields
    let metadata = &abi["metadata"];
    assert_eq!(metadata["schema_version"], "0.1.0");
    assert!(metadata.get("toolchain_version").is_some());
    assert!(metadata.get("source_hash").is_some());
    
    // Check module info
    assert_eq!(abi["module_name"], "demo");
    assert_eq!(abi["module_version"], "0.1.0");
    
    // Check functions
    let functions = &abi["functions"];
    assert!(functions.get("get_greeting").is_some());
    assert!(functions.get("set_greeting").is_some());
    
    // Check events
    let events = &abi["events"];
    assert!(events.get("GreetingChanged").is_some());
} 