#![allow(non_snake_case)]

use claims::{assert_none, assert_ok};

use super::*;
use crate::entities::Data;
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
    fn children_of() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let mut element = Element::new(&Path::new("::root::node").unwrap());
        assert!(interface.save(element.id(), &mut element).unwrap());
        assert_eq!(interface.children_of(&element).unwrap(), vec![]);

        let mut child1 = Element::new(&Path::new("::root::node::leaf1").unwrap());
        let mut child2 = Element::new(&Path::new("::root::node::leaf2").unwrap());
        let mut child3 = Element::new(&Path::new("::root::node::leaf3").unwrap());
        assert!(interface.save(child1.id(), &mut child1).unwrap());
        assert!(interface.save(child2.id(), &mut child2).unwrap());
        assert!(interface.save(child3.id(), &mut child3).unwrap());
        element.child_ids = vec![child1.id(), child2.id(), child3.id()];
        assert!(interface.save(element.id(), &mut element).unwrap());
        assert_eq!(
            interface.children_of(&element).unwrap(),
            vec![child1, child2, child3]
        );
    }

    #[test]
    fn find_by_id__existent() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let mut element = Element::new(&Path::new("::root::node::leaf").unwrap());
        let id = element.id();
        assert!(interface.save(id, &mut element).unwrap());

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
        let mut element = Element::new(&Path::new("::root::node::leaf").unwrap());

        assert_ok!(interface.save(element.id(), &mut element));
    }

    #[test]
    fn test_save__multiple() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let mut element1 = Element::new(&Path::new("::root::node1").unwrap());
        let mut element2 = Element::new(&Path::new("::root::node2").unwrap());

        assert!(interface.save(element1.id(), &mut element1).unwrap());
        assert!(interface.save(element2.id(), &mut element2).unwrap());
        assert_eq!(interface.find_by_id(element1.id()).unwrap(), Some(element1));
        assert_eq!(interface.find_by_id(element2.id()).unwrap(), Some(element2));
    }

    #[test]
    fn test_save__not_dirty() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let mut element = Element::new(&Path::new("::root::node::leaf").unwrap());
        let id = element.id();

        assert!(interface.save(id, &mut element).unwrap());
        element.update_data(Data {});
        assert!(interface.save(id, &mut element).unwrap());
    }

    #[test]
    fn test_save__too_old() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let mut element1 = Element::new(&Path::new("::root::node::leaf").unwrap());
        let mut element2 = element1.clone();
        let id = element1.id();

        assert!(interface.save(id, &mut element1).unwrap());
        element1.update_data(Data {});
        element2.update_data(Data {});
        assert!(interface.save(id, &mut element2).unwrap());
        assert!(!interface.save(id, &mut element1).unwrap());
    }

    #[test]
    fn test_save__update_existing() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let mut element = Element::new(&Path::new("::root::node::leaf").unwrap());
        let id = element.id();
        assert!(interface.save(id, &mut element).unwrap());

        // TODO: Modify the element's data and check it changed

        assert!(interface.save(id, &mut element).unwrap());
        assert_eq!(interface.find_by_id(id).unwrap(), Some(element));
    }

    #[test]
    #[ignore]
    fn validate() {
        todo!()
    }
}
