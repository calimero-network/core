use calimero_storage::address::Path;
use calimero_storage::entities::{Data, Element};
use calimero_storage::interface::Interface;
use calimero_storage_macros::{AtomicUnit, Collection};

#[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[type_id(2)]
struct Child {
    #[storage]
    storage: Element,
}

#[derive(Collection, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[children(Child)]
struct Group;

#[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[root]
#[type_id(1)]
struct Parent {
    group: Group,
	#[storage]
	storage: Element,
}

fn main() {
    fn child_type_specification() {
        let parent: Parent = Parent {
            group: Group {},
            storage: Element::new(&Path::new("::root::node").unwrap()),
        };
        let _: Vec<Child> = Interface::children_of(parent.id(), &parent.group).unwrap();

        // This should fail to compile if the child type is incorrect
        let _: Vec<Parent> = Interface::children_of(parent.id(), &parent.group).unwrap();
    }
}
