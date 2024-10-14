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
use calimero_storage::entities::{Data, Element, NoChildren};
use calimero_storage::interface::Interface;
use calimero_storage_macros::AtomicUnit;
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

#[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[children(Child)]
struct Parent {
    title: String,
    #[storage]
    storage: Element,
}

impl Parent {
    fn new(path: &Path) -> Self {
        Self {
            title: String::new(),
            storage: Element::new(path),
        }
    }
}

#[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
struct Private {
    public: String,
    #[private]
    private: String,
    #[storage]
    storage: Element,
}

impl Private {
    fn new(path: &Path) -> Self {
        Self {
            public: String::new(),
            private: String::new(),
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

#[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
struct Skipped {
    included: String,
    #[skip]
    skipped: String,
    #[storage]
    storage: Element,
}

impl Skipped {
    fn new(path: &Path) -> Self {
        Self {
            included: String::new(),
            skipped: String::new(),
            storage: Element::new(path),
        }
    }
}

#[cfg(test)]
mod basics {
    use super::*;

    #[test]
    fn creation() {
        let path = Path::new("::root::node").unwrap();
        let unit = Simple::new(&path);

        assert_eq!(unit.path(), path);
        assert_eq!(unit.element().path(), path);
        assert!(unit.element().is_dirty());
    }

    #[test]
    fn getters() {
        let path = Path::new("::root::node").unwrap();
        let unit = Simple::new(&path);

        assert_eq!(unit.name(), "");
        assert_eq!(unit.value(), &0);
    }

    #[test]
    fn setters__set() {
        let path = Path::new("::root::node").unwrap();
        let mut unit = Simple::new(&path);

        _ = unit.set_name("Test Name".to_owned());
        _ = unit.set_value(42);

        assert_eq!(unit.name(), "Test Name");
        assert_eq!(unit.value(), &42);
    }

    #[test]
    fn setters__confirm_set() {
        let path = Path::new("::root::node").unwrap();
        let mut unit = Simple::new(&path);
        assert_ne!(unit.name(), "Test Name");
        assert_ne!(unit.value(), &42);

        assert!(unit.set_name("Test Name".to_owned()));
        assert!(unit.set_value(42));
        assert_eq!(unit.name(), "Test Name");
        assert_eq!(unit.value(), &42);
    }

    #[test]
    fn setters__confirm_not_set() {
        let path = Path::new("::root::node").unwrap();
        let mut unit = Simple::new(&path);
        assert_ne!(unit.name(), "Test Name");
        assert_ne!(unit.value(), &42);

        assert!(unit.set_name("Test Name".to_owned()));
        assert!(unit.set_value(42));
        assert_eq!(unit.name(), "Test Name");
        assert_eq!(unit.value(), &42);
        assert!(!unit.set_name("Test Name".to_owned()));
        assert!(!unit.set_value(42));
    }

    #[test]
    fn setters__confirm_set_dirty() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let path = Path::new("::root::node").unwrap();
        let mut unit = Simple::new(&path);
        assert!(interface.save(unit.id(), &mut unit).unwrap());
        assert!(!unit.element().is_dirty());

        assert!(unit.set_name("Test Name".to_owned()));
        assert!(unit.element().is_dirty());
    }

    #[test]
    fn setters__confirm_not_set_not_dirty() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let path = Path::new("::root::node").unwrap();
        let mut unit = Simple::new(&path);
        assert!(interface.save(unit.id(), &mut unit).unwrap());
        assert!(!unit.element().is_dirty());

        assert!(unit.set_name("Test Name".to_owned()));
        assert!(unit.element().is_dirty());
        assert!(interface.save(unit.id(), &mut unit).unwrap());
        assert!(!unit.set_name("Test Name".to_owned()));
        assert!(!unit.element().is_dirty());
    }
}

#[cfg(test)]
mod visibility {
    use super::*;

    #[test]
    fn private_field() {
        let path = Path::new("::root::node").unwrap();
        let mut unit = Private::new(&path);

        _ = unit.set_public("Public".to_owned());
        _ = unit.set_private("Private".to_owned());

        let serialized = to_vec(&unit).unwrap();
        let deserialized = Private::try_from_slice(&serialized).unwrap();

        assert_eq!(unit.public(), deserialized.public());
        assert_ne!(unit.private(), deserialized.private());
        assert_eq!(deserialized.private(), "");
    }

    #[test]
    fn public_field() {
        let path = Path::new("::root::node").unwrap();
        let mut unit = Simple::new(&path);

        _ = unit.set_name("Public".to_owned());

        let serialized = to_vec(&unit).unwrap();
        let deserialized = Simple::try_from_slice(&serialized).unwrap();

        assert_eq!(unit.name(), deserialized.name());
    }

    #[test]
    fn skipped_field() {
        let path = Path::new("::root::node").unwrap();
        let mut unit = Skipped::new(&path);

        _ = unit.set_included("Public".to_owned());
        // Skipping fields also skips the setters
        // _ = unit.set_skipped("Skipped".to_owned());
        unit.skipped = "Skipped".to_owned();

        let serialized = to_vec(&unit).unwrap();
        let deserialized = Skipped::try_from_slice(&serialized).unwrap();

        assert_eq!(unit.included(), deserialized.included());
        // Skipping fields also skips the getters
        // assert_ne!(unit.skipped(), deserialized.skipped());
        assert_ne!(unit.skipped, deserialized.skipped);
        assert_eq!(deserialized.skipped, "");
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
    fn no_children() {
        let _unit: Simple = Simple::new(&Path::new("::root::node").unwrap());
        let _: Option<<Simple as Data>::Child> = None::<NoChildren>;
    }

    #[test]
    fn compile_fail() {
        trybuild::TestCases::new().compile_fail("tests/compile_fail/atomic_unit.rs");
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
