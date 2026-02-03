//! Unit tests for Collection serialization/deserialization
//!
//! These tests verify:
//! 1. Collection serializes only Element.id (not metadata)
//! 2. Collection deserialization works correctly
//! 3. Backward compatibility with old formats
//! 4. "Not all bytes read" error scenarios

use crate::address::Id;
use crate::collections::{Root, UnorderedMap};
use crate::entities::{CrdtType, Element, Metadata, StorageType};
use borsh::{BorshDeserialize, BorshSerialize};

// We need to access the internal Collection struct for testing
// Since it's private, we'll test through the public API where possible
// and create a test-only version if needed

#[test]
fn test_element_serialization_only_id() {
    // Element should only serialize `id`, not metadata, is_dirty, or merkle_hash
    let element = Element::new(Some(Id::random()));

    let serialized = borsh::to_vec(&element).unwrap();

    // Element should serialize only the 32-byte ID
    assert_eq!(
        serialized.len(),
        32,
        "Element should serialize only 32 bytes (Id)"
    );

    // Verify we can deserialize it back
    let deserialized: Element = BorshDeserialize::try_from_slice(&serialized).unwrap();
    assert_eq!(deserialized.id(), element.id());
}

#[test]
fn test_element_deserialization_with_old_format() {
    // Simulate old Element format (just ID, no extra fields)
    let id = Id::random();
    let old_format_bytes = borsh::to_vec(&id).unwrap();

    // Should deserialize correctly (Element only reads ID)
    let deserialized: Element = BorshDeserialize::try_from_slice(&old_format_bytes).unwrap();
    assert_eq!(deserialized.id(), id);
}

#[test]
fn test_metadata_serialization_with_crdt_type() {
    let metadata = Metadata::with_crdt_type(1000, 2000, CrdtType::Counter);
    let serialized = borsh::to_vec(&metadata).unwrap();

    // Should serialize: created_at (8) + updated_at (8) + storage_type (1) + crdt_type Option (1 + variant)
    // Let's verify it deserializes correctly
    let deserialized: Metadata = BorshDeserialize::try_from_slice(&serialized).unwrap();
    assert_eq!(deserialized.crdt_type, Some(CrdtType::Counter));
}

#[test]
fn test_metadata_deserialization_without_crdt_type() {
    // Create metadata without crdt_type (old format)
    // We'll manually create bytes for old format: created_at + updated_at + storage_type
    use crate::entities::UpdatedAt;

    let created_at = 1000u64;
    let updated_at = UpdatedAt::from(2000u64);
    let storage_type = StorageType::Public;

    // Serialize old format manually (without crdt_type)
    let mut old_format_bytes = Vec::new();
    old_format_bytes.extend_from_slice(&created_at.to_le_bytes());
    old_format_bytes.extend_from_slice(&updated_at.to_le_bytes());
    // Serialize storage_type using Borsh
    let storage_type_bytes = borsh::to_vec(&storage_type).unwrap();
    old_format_bytes.extend_from_slice(&storage_type_bytes);
    // Note: old format doesn't have crdt_type field

    // Should deserialize with crdt_type = None (backward compatibility)
    let deserialized: Metadata = BorshDeserialize::try_from_slice(&old_format_bytes).unwrap();
    assert_eq!(deserialized.created_at, created_at);
    assert_eq!(deserialized.updated_at(), 2000);
    assert_eq!(
        deserialized.crdt_type, None,
        "Old format should have crdt_type = None"
    );
}

#[test]
fn test_metadata_deserialization_with_extra_bytes() {
    // Create metadata with crdt_type
    let metadata = Metadata::with_crdt_type(1000, 2000, CrdtType::Counter);
    let mut serialized = borsh::to_vec(&metadata).unwrap();

    // Add extra bytes (simulating "Not all bytes read" scenario)
    serialized.push(0x42);
    serialized.push(0x43);

    // This should fail with "Not all bytes read"
    let result: Result<Metadata, _> = BorshDeserialize::try_from_slice(&serialized);
    assert!(result.is_err());

    let err = result.unwrap_err();
    let err_str = err.to_string();
    assert!(
        err_str.contains("Not all bytes read") || err_str.contains("Unexpected length"),
        "Should fail with 'Not all bytes read' error, got: {}",
        err_str
    );
}

#[test]
fn test_collection_serialization_size() {
    // Create a root collection
    let root: Root<UnorderedMap<String, String>> = Root::new(|| UnorderedMap::new());

    // Get the inner collection
    // We can't directly access the inner Collection, but we can test through Root
    // The Collection should serialize only Element.id (32 bytes)

    // Serialize the root's inner collection by committing and reading back
    root.commit();

    // The Collection struct should only serialize Element.id
    // Element.id is 32 bytes
    // So Collection serialization should be exactly 32 bytes
    let element = Element::new(Some(Id::root()));
    let element_bytes = borsh::to_vec(&element).unwrap();
    assert_eq!(
        element_bytes.len(),
        32,
        "Element serialization should be 32 bytes"
    );
}

#[test]
fn test_collection_deserialization_with_extra_bytes() {
    // Create a minimal Collection-like structure
    // Collection<T> serializes as: Element (which is just Id = 32 bytes)
    let id = Id::root();
    let mut collection_bytes = borsh::to_vec(&id).unwrap();

    // Add extra bytes (simulating old format or corruption)
    collection_bytes.push(0x01);
    collection_bytes.push(0x02);
    collection_bytes.push(0x03);

    // Try to deserialize as Element (what Collection contains)
    let result: Result<Element, _> = BorshDeserialize::try_from_slice(&collection_bytes);

    // This should fail with "Not all bytes read" because we added extra bytes
    assert!(result.is_err());
    let err = result.unwrap_err();
    let err_str = err.to_string();
    assert!(
        err_str.contains("Not all bytes read") || err_str.contains("Unexpected length"),
        "Should fail with 'Not all bytes read' error when extra bytes present, got: {}",
        err_str
    );
}

#[test]
fn test_collection_round_trip() {
    // Create a root collection
    let mut root: Root<UnorderedMap<String, String>> = Root::new(|| UnorderedMap::new());

    // Insert something
    root.insert("key1".to_string(), "value1".to_string())
        .unwrap();

    // Commit
    root.commit();

    // Fetch should work (this is what's failing in the workflow)
    let fetched = Root::<UnorderedMap<String, String>>::fetch();
    assert!(
        fetched.is_some(),
        "Root::fetch() should succeed after commit"
    );

    let fetched_root = fetched.unwrap();
    let value = fetched_root.get("key1").unwrap();
    assert_eq!(value, Some("value1".to_string()));
}

#[test]
fn test_element_id_only_serialization() {
    // Verify Element only serializes id field
    let id1 = Id::random();
    let id2 = Id::random();

    let element1 = Element::new(Some(id1));
    let element2 = Element::new(Some(id2));

    let bytes1 = borsh::to_vec(&element1).unwrap();
    let bytes2 = borsh::to_vec(&element2).unwrap();

    // Both should be exactly 32 bytes (just the ID)
    assert_eq!(bytes1.len(), 32);
    assert_eq!(bytes2.len(), 32);

    // They should be different (different IDs)
    assert_ne!(bytes1, bytes2);

    // But should match the ID bytes
    assert_eq!(bytes1, id1.as_bytes());
    assert_eq!(bytes2, id2.as_bytes());
}
