#![allow(non_snake_case)]

use claims::{assert_none, assert_ok};

use super::*;
use crate::tests::common::create_test_store;

#[cfg(test)]
mod interface__constructor {
    use super::*;

    #[test]
    fn new() {
        let (db, _dir) = create_test_store();
        drop(Interface::new(db));
    }
}

#[cfg(test)]
mod interface__public_methods {
    use super::*;

    #[test]
    fn find_by_id__existent() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let element = Element::new(&Path::new("::root::node::leaf").unwrap());
        let id = element.id();
        interface.save(id, &element).unwrap();

        assert_eq!(interface.find_by_id(id).unwrap(), Some(element));
    }

    #[test]
    fn find_by_id__non_existent() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);

        assert_none!(interface.find_by_id(Id::new()).unwrap());
    }

    #[test]
    #[ignore]
    fn find_by_path() {
        todo!()
    }

    #[test]
    #[ignore]
    fn find_children_by_id() {
        todo!()
    }

    #[test]
    fn test_save__basic() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let element = Element::new(&Path::new("::root::node::leaf").unwrap());

        assert_ok!(interface.save(element.id(), &element));
    }

    #[test]
    fn test_save__multiple() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let element1 = Element::new(&Path::new("::root::node1").unwrap());
        let element2 = Element::new(&Path::new("::root::node2").unwrap());

        interface.save(element1.id(), &element1).unwrap();
        interface.save(element2.id(), &element2).unwrap();
        assert_eq!(interface.find_by_id(element1.id()).unwrap(), Some(element1));
        assert_eq!(interface.find_by_id(element2.id()).unwrap(), Some(element2));
    }

    #[test]
    fn test_save__update_existing() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let element = Element::new(&Path::new("::root::node::leaf").unwrap());
        let id = element.id();
        interface.save(id, &element).unwrap();

        // TODO: Modify the element's data and check it changed

        interface.save(id, &element).unwrap();
        assert_eq!(interface.find_by_id(id).unwrap(), Some(element));
    }

    #[test]
    #[ignore]
    fn validate() {
        todo!()
    }
}
