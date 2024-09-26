use calimero_storage::address::Path;
use calimero_storage::entities::Element;
use calimero_storage::interface::Interface;
use calimero_storage_macros::AtomicUnit;
use calimero_test_utils::storage::create_test_store;

#[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
struct Child {
    #[storage]
    storage: Element,
}

#[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[children(Child)]
struct Parent {
	#[storage]
	storage: Element,
}

fn main() {
    fn child_type_specification() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let parent: Parent = Parent { storage: Element::new(&Path::new("::root::node").unwrap()) };
        let _: Vec<Child> = interface.children_of(&parent).unwrap();

        // This should fail to compile if the child type is incorrect
        let _: Vec<Parent> = interface.children_of(&parent).unwrap();
    }
}
