#![allow(unused_crate_dependencies, reason = "Creates a lot of noise")]
//	Lints specifically disabled for integration tests
#![allow(
    non_snake_case,
    unreachable_pub,
    clippy::cast_lossless,
    clippy::cast_precision_loss,
    clippy::cognitive_complexity,
    clippy::default_numeric_fallback,
    clippy::exhaustive_enums,
    clippy::exhaustive_structs,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::missing_assert_message,
    clippy::missing_panics_doc,
    clippy::mod_module_files,
    clippy::must_use_candidate,
    clippy::panic,
    clippy::print_stdout,
    clippy::tests_outside_test_module,
    clippy::unwrap_in_result,
    clippy::unwrap_used,
    reason = "Not useful in tests"
)]

use borsh::{to_vec, BorshDeserialize};
use calimero_storage::address::{Id, Path};
use calimero_storage::entities::{Data, Element};
use calimero_storage::exports::{Digest, Sha256};
use calimero_storage::interface::Interface;
use calimero_storage_macros::{AtomicUnit, Collection};
use calimero_test_utils::storage::create_test_store;

#[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
struct Child {
    content: String,
    #[storage]
    storage: Element,
}

impl Child {
    fn new(path: &Path) -> Self {
        Self {
            content: String::new(),
            storage: Element::new(path),
        }
    }
}

#[derive(Collection, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[children(Child)]
struct Group {
    #[child_ids]
    child_ids: Vec<Id>,
}

impl Group {
    fn new() -> Self {
        Self {
            child_ids: Vec::new(),
        }
    }
}

#[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
struct Parent {
    title: String,
    #[collection]
    children: Group,
    #[storage]
    storage: Element,
}

impl Parent {
    fn new(path: &Path) -> Self {
        Self {
            title: String::new(),
            children: Group::new(),
            storage: Element::new(path),
        }
    }
}

#[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
struct Simple {
    name: String,
    value: i32,
    #[storage]
    storage: Element,
}

impl Simple {
    fn new(path: &Path) -> Self {
        Self {
            name: String::new(),
            value: 0,
            storage: Element::new(path),
        }
    }
}

#[cfg(test)]
mod hierarchy {
    use super::*;

    #[test]
    fn parent_child() {
        let parent_path = Path::new("::root::node").unwrap();
        let mut parent = Parent::new(&parent_path);
        _ = parent.set_title("Parent Title".to_owned());

        let child_path = Path::new("::root::node::leaf").unwrap();
        let mut child = Child::new(&child_path);
        _ = child.set_content("Child Content".to_owned());

        assert_eq!(parent.title(), "Parent Title");

        // TODO: Add in tests for loading and checking children
    }

    #[test]
    fn compile_fail() {
        trybuild::TestCases::new().compile_fail("tests/compile_fail/collection.rs");
    }
}

#[cfg(test)]
mod hashing {
    use super::*;

    #[test]
    fn calculate_full_merkle_hash__cached_values() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let mut parent = Parent::new(&Path::new("::root::node").unwrap());
        _ = parent.set_title("Parent Title".to_owned());
        assert!(interface.save(parent.id(), &mut parent).unwrap());
        assert_eq!(interface.children_of(&parent.children).unwrap(), vec![]);

        let mut child1 = Child::new(&Path::new("::root::node::leaf1").unwrap());
        let mut child2 = Child::new(&Path::new("::root::node::leaf2").unwrap());
        let mut child3 = Child::new(&Path::new("::root::node::leaf3").unwrap());
        _ = child1.set_content("Child 1 Content".to_owned());
        _ = child2.set_content("Child 2 Content".to_owned());
        _ = child3.set_content("Child 3 Content".to_owned());
        assert!(interface.save(child1.id(), &mut child1).unwrap());
        assert!(interface.save(child2.id(), &mut child2).unwrap());
        assert!(interface.save(child3.id(), &mut child3).unwrap());
        parent.children.child_ids = vec![child1.id(), child2.id(), child3.id()];
        assert!(interface.save(parent.id(), &mut parent).unwrap());

        let mut hasher0 = Sha256::new();
        hasher0.update(parent.id().as_bytes());
        hasher0.update(&to_vec(&parent.title).unwrap());
        hasher0.update(&to_vec(&parent.element().metadata()).unwrap());
        let expected_hash0: [u8; 32] = hasher0.finalize().into();

        let mut hasher1 = Sha256::new();
        hasher1.update(child1.id().as_bytes());
        hasher1.update(&to_vec(&child1.content).unwrap());
        hasher1.update(&to_vec(&child1.element().metadata()).unwrap());
        let expected_hash1: [u8; 32] = hasher1.finalize().into();
        let mut hasher1b = Sha256::new();
        hasher1b.update(expected_hash1);
        let expected_hash1b: [u8; 32] = hasher1b.finalize().into();

        let mut hasher2 = Sha256::new();
        hasher2.update(child2.id().as_bytes());
        hasher2.update(&to_vec(&child2.content).unwrap());
        hasher2.update(&to_vec(&child2.element().metadata()).unwrap());
        let expected_hash2: [u8; 32] = hasher2.finalize().into();
        let mut hasher2b = Sha256::new();
        hasher2b.update(expected_hash2);
        let expected_hash2b: [u8; 32] = hasher2b.finalize().into();

        let mut hasher3 = Sha256::new();
        hasher3.update(child3.id().as_bytes());
        hasher3.update(&to_vec(&child3.content).unwrap());
        hasher3.update(&to_vec(&child3.element().metadata()).unwrap());
        let expected_hash3: [u8; 32] = hasher3.finalize().into();
        let mut hasher3b = Sha256::new();
        hasher3b.update(expected_hash3);
        let expected_hash3b: [u8; 32] = hasher3b.finalize().into();

        let mut hasher = Sha256::new();
        hasher.update(&expected_hash0);
        hasher.update(&expected_hash1b);
        hasher.update(&expected_hash2b);
        hasher.update(&expected_hash3b);
        let expected_hash: [u8; 32] = hasher.finalize().into();

        assert_eq!(parent.calculate_merkle_hash().unwrap(), expected_hash0);
        assert_eq!(
            child1
                .calculate_full_merkle_hash(&interface, false)
                .unwrap(),
            expected_hash1b
        );
        assert_eq!(
            child2
                .calculate_full_merkle_hash(&interface, false)
                .unwrap(),
            expected_hash2b
        );
        assert_eq!(
            child3
                .calculate_full_merkle_hash(&interface, false)
                .unwrap(),
            expected_hash3b
        );
        assert_eq!(
            parent
                .calculate_full_merkle_hash(&interface, false)
                .unwrap(),
            expected_hash
        );
    }

    #[test]
    #[ignore]
    fn calculate_full_merkle_hash__recalculated_values() {
        // TODO: Later, tests should be added for recalculating the hashes, and
        // TODO: especially checking when the data has been interfered with or
        // TODO: otherwise arrived at an invalid state.
        todo!()
    }

    #[test]
    fn calculate_merkle_hash__child() {
        let mut child = Child::new(&Path::new("::root::node::leaf").unwrap());
        _ = child.set_content("Child Content".to_owned());

        let mut hasher = Sha256::new();
        hasher.update(child.id().as_bytes());
        hasher.update(&to_vec(&child.content).unwrap());
        hasher.update(&to_vec(&child.element().metadata()).unwrap());
        let expected_hash: [u8; 32] = hasher.finalize().into();

        assert_eq!(child.calculate_merkle_hash().unwrap(), expected_hash);
    }

    #[test]
    fn calculate_merkle_hash__parent() {
        let mut parent = Parent::new(&Path::new("::root::node").unwrap());
        _ = parent.set_title("Parent Title".to_owned());

        let mut hasher = Sha256::new();
        hasher.update(parent.id().as_bytes());
        hasher.update(&to_vec(&parent.title).unwrap());
        hasher.update(&to_vec(&parent.element().metadata()).unwrap());
        let expected_hash: [u8; 32] = hasher.finalize().into();

        assert_eq!(parent.calculate_merkle_hash().unwrap(), expected_hash);
    }
}

#[cfg(test)]
mod traits {
    use super::*;

    #[test]
    fn serialization_and_deserialization() {
        let path = Path::new("::root::node").unwrap();
        let mut unit = Simple::new(&path);

        _ = unit.set_name("Test Name".to_owned());
        _ = unit.set_value(42);

        let serialized = to_vec(&unit).unwrap();
        let deserialized = Simple::try_from_slice(&serialized).unwrap();

        assert_eq!(unit, deserialized);
        assert_eq!(unit.id(), deserialized.id());
        assert_eq!(unit.name(), deserialized.name());
        assert_eq!(unit.path(), deserialized.path());
        assert_eq!(unit.value(), deserialized.value());
        assert_eq!(unit.element().id(), deserialized.element().id());
        assert_eq!(unit.element().path(), deserialized.element().path());
        assert_eq!(unit.element().metadata(), deserialized.element().metadata());
    }
}
