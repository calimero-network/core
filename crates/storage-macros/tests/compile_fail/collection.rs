use calimero_storage::address::Path;
use calimero_storage::entities::{ChildInfo, Element};
use calimero_storage::interface::Interface;
use calimero_storage_macros::{AtomicUnit, Collection};

#[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
struct Child {
    #[storage]
    storage: Element,
}

#[derive(Collection, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[children(Child)]
struct Group {
    #[child_info]
    child_info: Vec<ChildInfo>,
}

#[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
struct Parent {
    group: Group,
	#[storage]
	storage: Element,
}

fn main() {
    fn child_type_specification() {
        let interface = Interface::new();
        let parent: Parent = Parent {
            group: Group { child_info: vec![] },
            storage: Element::new(&Path::new("::root::node").unwrap()),
        };
        let _: Vec<Child> = interface.children_of(&parent.group).unwrap();

        // This should fail to compile if the child type is incorrect
        let _: Vec<Parent> = interface.children_of(&parent.group).unwrap();
    }
}
