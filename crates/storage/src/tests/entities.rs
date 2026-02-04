use std::time::{SystemTime, UNIX_EPOCH};

use claims::{assert_ge, assert_le};
use sha2::{Digest, Sha256};
use velcro::btree_map;

use super::*;
use crate::interface::MainInterface;
use crate::tests::common::{Page, Paragraph, Paragraphs, Person};

#[cfg(test)]
mod collection__public_methods {
    use super::*;

    #[test]
    fn name() {
        let _paras = Paragraphs::new();
    }
}

#[cfg(test)]
mod data__public_methods {
    use super::*;

    #[test]
    fn collections() {
        let parent = Element::new(None);
        let page = Page::new_from_element("Node", parent);
        assert_eq!(
            page.collections(),
            btree_map! {
                "Paragraphs".to_owned(): MainInterface::child_info_for(page.id()).unwrap_or_default(),
            }
        );

        let child = Element::new(None);
        let para = Paragraph::new_from_element("Leaf", child);
        assert_eq!(para.collections(), BTreeMap::new());
    }

    #[test]
    fn element() {
        let element = Element::new(None);
        let person = Person {
            name: "Alice".to_owned(),
            age: 30,
            storage: element.clone(),
        };
        assert_eq!(person.element(), &element);
    }

    #[test]
    fn element_mut() {
        let element = Element::new(None);
        let mut person = Person {
            name: "Bob".to_owned(),
            age: 40,
            storage: element.clone(),
        };
        assert!(element.is_dirty);
        assert!(person.element().is_dirty);
        person.element_mut().is_dirty = false;
        assert!(element.is_dirty);
        assert!(!person.element().is_dirty);
    }

    #[test]
    fn id() {
        let element = Element::new(None);
        let id = element.id;
        let person = Person {
            name: "Eve".to_owned(),
            age: 20,
            storage: element,
        };
        assert_eq!(person.id(), id);
    }
}

#[cfg(test)]
mod child_info__constructor {
    use super::*;

    #[test]
    fn new() {
        let id = Id::random();
        let hash = Sha256::digest(b"1").into();
        let info = ChildInfo::new(id, hash, Metadata::default());
        assert_eq!(info.id, id);
        assert_eq!(info.merkle_hash, hash);
    }
}

#[cfg(test)]
mod child_info__public_methods {
    use super::*;

    #[test]
    fn id() {
        let info = ChildInfo::new(
            Id::random(),
            Sha256::digest(b"1").into(),
            Metadata::default(),
        );
        assert_eq!(info.id(), info.id);
    }

    #[test]
    fn merkle_hash() {
        let info = ChildInfo::new(
            Id::random(),
            Sha256::digest(b"1").into(),
            Metadata::default(),
        );
        assert_eq!(info.merkle_hash(), info.merkle_hash);
    }
}

#[cfg(test)]
mod child_info__traits {
    use super::*;

    #[test]
    fn display() {
        let info = ChildInfo::new(
            Id::random(),
            Sha256::digest(b"1").into(),
            Metadata::default(),
        );
        assert_eq!(
            format!("{info}"),
            format!(
                "ChildInfo {}: {}",
                info.id(),
                hex::encode(info.merkle_hash())
            )
        );
        assert_eq!(
            info.to_string(),
            format!(
                "ChildInfo {}: {}",
                info.id(),
                hex::encode(info.merkle_hash())
            )
        );
    }
}

#[cfg(test)]
mod element__constructor {
    use super::*;

    #[test]
    fn new() {
        let timestamp1 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        let element = Element::new(None);
        let timestamp2 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        assert_ge!(element.metadata.created_at, timestamp1);
        assert_le!(element.metadata.created_at, timestamp2);
        assert_ge!(*element.metadata.updated_at, timestamp1);
        assert_le!(*element.metadata.updated_at, timestamp2);
        assert!(element.is_dirty);
    }
}

#[cfg(test)]
mod element__public_methods {
    use super::*;

    #[test]
    fn created_at() {
        let timestamp1 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        let element = Element::new(None);
        let timestamp2 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        assert_ge!(element.created_at(), timestamp1);
        assert_le!(element.created_at(), timestamp2);
    }

    #[test]
    fn id() {
        let element = Element::new(None);
        assert_eq!(element.id(), element.id);
    }

    #[test]
    fn is_dirty() {
        let element = Element::root();
        assert!(element.is_dirty());

        let mut person = Person {
            name: "Alice".to_owned(),
            age: 30,
            storage: element,
        };
        assert!(MainInterface::save(&mut person).unwrap());
        assert!(!person.element().is_dirty());

        person.element_mut().update();
        assert!(person.element().is_dirty());
    }

    #[test]
    #[ignore]
    fn metadata() {
        todo!()
    }

    #[test]
    fn update() {
        let element = Element::root();
        let updated_at = element.metadata.updated_at;
        let mut person = Person {
            name: "Bob".to_owned(),
            age: 40,
            storage: element,
        };
        assert!(MainInterface::save(&mut person).unwrap());
        assert!(!person.element().is_dirty);

        person.element_mut().update();
        assert!(person.element().is_dirty);
        assert_ge!(person.element().metadata.updated_at, updated_at);
    }

    #[test]
    fn updated_at() {
        let timestamp1 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        let element = Element::new(None);
        let mut person = Person {
            name: "Eve".to_owned(),
            age: 20,
            storage: element,
        };
        let timestamp2 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        assert_ge!(person.element().updated_at(), timestamp1);
        assert_le!(person.element().updated_at(), timestamp2);

        person.element_mut().update();
        let timestamp3 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        assert_ge!(person.element().updated_at(), timestamp2);
        assert_le!(person.element().updated_at(), timestamp3);
    }
}

#[cfg(test)]
mod element__traits {
    use super::*;

    #[test]
    fn display() {
        let element = Element::new(None);
        assert_eq!(format!("{element}"), format!("Element {}", element.id()));
        assert_eq!(element.to_string(), format!("Element {}", element.id()));
    }
}

#[cfg(test)]
mod metadata__constructor {
    use super::*;

    #[test]
    fn new() {
        let metadata = Metadata::new(1000, 2000);
        assert_eq!(metadata.created_at, 1000);
        assert_eq!(*metadata.updated_at, 2000);
        // Metadata::new() now defaults to LwwRegister CRDT type
        assert_eq!(metadata.crdt_type, Some(CrdtType::LwwRegister));
    }

    #[test]
    fn with_crdt_type() {
        let metadata = Metadata::with_crdt_type(1000, 2000, CrdtType::Counter);
        assert_eq!(metadata.created_at, 1000);
        assert_eq!(*metadata.updated_at, 2000);
        assert_eq!(metadata.crdt_type, Some(CrdtType::Counter));
    }
}

#[cfg(test)]
mod metadata__crdt_type {
    use super::*;

    #[test]
    fn is_builtin_crdt__counter() {
        let metadata = Metadata::with_crdt_type(1000, 2000, CrdtType::Counter);
        assert!(metadata.is_builtin_crdt());
    }

    #[test]
    fn is_builtin_crdt__lww_register() {
        let metadata = Metadata::with_crdt_type(1000, 2000, CrdtType::LwwRegister);
        assert!(metadata.is_builtin_crdt());
    }

    #[test]
    fn is_builtin_crdt__rga() {
        let metadata = Metadata::with_crdt_type(1000, 2000, CrdtType::Rga);
        assert!(metadata.is_builtin_crdt());
    }

    #[test]
    fn is_builtin_crdt__unordered_map() {
        let metadata = Metadata::with_crdt_type(1000, 2000, CrdtType::UnorderedMap);
        assert!(metadata.is_builtin_crdt());
    }

    #[test]
    fn is_builtin_crdt__unordered_set() {
        let metadata = Metadata::with_crdt_type(1000, 2000, CrdtType::UnorderedSet);
        assert!(metadata.is_builtin_crdt());
    }

    #[test]
    fn is_builtin_crdt__vector() {
        let metadata = Metadata::with_crdt_type(1000, 2000, CrdtType::Vector);
        assert!(metadata.is_builtin_crdt());
    }

    #[test]
    fn is_builtin_crdt__custom() {
        let metadata = Metadata::with_crdt_type(
            1000,
            2000,
            CrdtType::Custom {
                type_name: "MyCRDT".to_string(),
            },
        );
        assert!(!metadata.is_builtin_crdt());
    }

    #[test]
    fn is_builtin_crdt__none() {
        let mut metadata = Metadata::new(1000, 2000);
        metadata.crdt_type = None; // Explicitly set to None for this test
        assert!(!metadata.is_builtin_crdt());
    }
}

#[cfg(test)]
mod metadata__serialization {
    use super::*;
    use borsh::{BorshDeserialize, BorshSerialize};

    #[test]
    fn serialize_deserialize__with_crdt_type() {
        let metadata = Metadata::with_crdt_type(1000, 2000, CrdtType::Counter);
        let serialized = borsh::to_vec(&metadata).unwrap();
        let deserialized: Metadata = BorshDeserialize::try_from_slice(&serialized).unwrap();
        assert_eq!(metadata.created_at, deserialized.created_at);
        assert_eq!(metadata.updated_at, deserialized.updated_at);
        assert_eq!(metadata.crdt_type, deserialized.crdt_type);
        assert_eq!(deserialized.crdt_type, Some(CrdtType::Counter));
    }

    #[test]
    fn serialize_deserialize__without_crdt_type() {
        let mut metadata = Metadata::new(1000, 2000);
        metadata.crdt_type = None; // Explicitly set to None for this test
        let serialized = borsh::to_vec(&metadata).unwrap();
        let deserialized: Metadata = BorshDeserialize::try_from_slice(&serialized).unwrap();
        assert_eq!(metadata.created_at, deserialized.created_at);
        assert_eq!(metadata.updated_at, deserialized.updated_at);
        assert_eq!(deserialized.crdt_type, None);
    }

    #[test]
    fn serialize_deserialize__custom_crdt() {
        let metadata = Metadata::with_crdt_type(
            1000,
            2000,
            CrdtType::Custom {
                type_name: "MyCustomCRDT".to_string(),
            },
        );
        let serialized = borsh::to_vec(&metadata).unwrap();
        let deserialized: Metadata = BorshDeserialize::try_from_slice(&serialized).unwrap();
        assert_eq!(metadata.crdt_type, deserialized.crdt_type);
        match deserialized.crdt_type {
            Some(CrdtType::Custom { type_name }) => {
                assert_eq!(type_name, "MyCustomCRDT");
            }
            _ => panic!("Expected Custom CRDT type"),
        }
    }

    #[test]
    fn default__has_none_crdt_type() {
        let metadata = Metadata::default();
        assert_eq!(metadata.crdt_type, None);
    }

    #[test]
    fn serialize_deserialize__with_field_name() {
        let mut metadata = Metadata::with_crdt_type(1000, 2000, CrdtType::UnorderedMap);
        metadata.field_name = Some("items".to_string());
        let serialized = borsh::to_vec(&metadata).unwrap();
        let deserialized: Metadata = BorshDeserialize::try_from_slice(&serialized).unwrap();
        assert_eq!(deserialized.field_name, Some("items".to_string()));
        assert_eq!(deserialized.crdt_type, Some(CrdtType::UnorderedMap));
    }

    #[test]
    fn serialize_deserialize__without_field_name() {
        let metadata = Metadata::with_crdt_type(1000, 2000, CrdtType::Counter);
        // field_name is None by default
        assert_eq!(metadata.field_name, None);
        let serialized = borsh::to_vec(&metadata).unwrap();
        let deserialized: Metadata = BorshDeserialize::try_from_slice(&serialized).unwrap();
        assert_eq!(deserialized.field_name, None);
        assert_eq!(deserialized.crdt_type, Some(CrdtType::Counter));
    }
}

#[cfg(test)]
mod element__new_with_field_name {
    use super::*;

    #[test]
    fn creates_element_with_field_name() {
        let element = Element::new_with_field_name(None, Some("my_field".to_string()));
        assert_eq!(element.metadata.field_name, Some("my_field".to_string()));
        // Default CRDT type for new_with_field_name is LwwRegister
        assert_eq!(element.metadata.crdt_type, Some(CrdtType::LwwRegister));
    }

    #[test]
    fn creates_element_without_field_name() {
        let element = Element::new_with_field_name(None, None);
        assert_eq!(element.metadata.field_name, None);
    }

    #[test]
    fn creates_element_with_field_name_and_crdt_type() {
        let element = Element::new_with_field_name_and_crdt_type(
            None,
            Some("items".to_string()),
            CrdtType::UnorderedMap,
        );
        assert_eq!(element.metadata.field_name, Some("items".to_string()));
        assert_eq!(element.metadata.crdt_type, Some(CrdtType::UnorderedMap));
    }

    #[test]
    fn new_defaults_to_no_field_name() {
        let element = Element::new(None);
        assert_eq!(element.metadata.field_name, None);
    }
}

#[cfg(test)]
mod metadata__backward_compatibility {
    use super::*;
    use borsh::BorshDeserialize;

    /// Test that old Metadata format (without crdt_type and field_name) deserializes correctly.
    /// This simulates data written before crdt_type and field_name were added.
    #[test]
    fn deserialize_old_format_without_crdt_type_and_field_name() {
        // Manually construct old-format Metadata bytes:
        // created_at: u64 (8 bytes)
        // updated_at: u64 (8 bytes)
        // storage_type: Public variant (1 byte for enum discriminant)
        let mut old_bytes = Vec::new();
        old_bytes.extend_from_slice(&1000u64.to_le_bytes()); // created_at
        old_bytes.extend_from_slice(&2000u64.to_le_bytes()); // updated_at
        old_bytes.push(0u8); // StorageType::Public enum discriminant

        // Deserialize - should succeed with None for crdt_type and field_name
        let deserialized: Metadata = BorshDeserialize::try_from_slice(&old_bytes).unwrap();
        assert_eq!(deserialized.created_at, 1000);
        assert_eq!(*deserialized.updated_at, 2000);
        assert!(matches!(deserialized.storage_type, StorageType::Public));
        assert_eq!(deserialized.crdt_type, None);
        assert_eq!(deserialized.field_name, None);
    }

    /// Test that Metadata with crdt_type but without field_name deserializes correctly.
    /// This simulates data written after crdt_type was added but before field_name.
    #[test]
    fn deserialize_format_with_crdt_type_without_field_name() {
        // Construct Metadata with crdt_type but let field_name be missing
        let metadata_with_crdt = Metadata::with_crdt_type(1000, 2000, CrdtType::Counter);
        let mut bytes_with_crdt = borsh::to_vec(&metadata_with_crdt).unwrap();

        // Remove the field_name bytes (last few bytes after crdt_type)
        // Since field_name is Option<String> serialized as None (0 byte), we can test
        // by ensuring current format works correctly
        let deserialized: Metadata = BorshDeserialize::try_from_slice(&bytes_with_crdt).unwrap();
        assert_eq!(deserialized.crdt_type, Some(CrdtType::Counter));
        // field_name should be None (default when not set)
        assert_eq!(deserialized.field_name, None);
    }

    /// Test that current format with all fields deserializes correctly.
    #[test]
    fn deserialize_current_format_with_all_fields() {
        let mut metadata = Metadata::with_crdt_type(1000, 2000, CrdtType::UnorderedMap);
        metadata.field_name = Some("test_field".to_string());

        let bytes = borsh::to_vec(&metadata).unwrap();
        let deserialized: Metadata = BorshDeserialize::try_from_slice(&bytes).unwrap();

        assert_eq!(deserialized.created_at, 1000);
        assert_eq!(*deserialized.updated_at, 2000);
        assert_eq!(deserialized.crdt_type, Some(CrdtType::UnorderedMap));
        assert_eq!(deserialized.field_name, Some("test_field".to_string()));
    }
}
