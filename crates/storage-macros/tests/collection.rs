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
use calimero_storage::address::Path;
use calimero_storage::entities::{Data, Element};
use calimero_storage::exports::{Digest, Sha256};
use calimero_storage_macros::{AtomicUnit, Collection};

#[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[type_id(22)]
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
struct Group;

impl Group {
    fn new() -> Self {
        Self {}
    }
}

#[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[type_id(21)]
#[root]
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
#[type_id(23)]
#[root]
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
