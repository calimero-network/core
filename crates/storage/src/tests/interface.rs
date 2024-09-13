#![allow(non_snake_case)]

use claims::{assert_none, assert_ok};

use super::*;
use crate::entities::Data;
use crate::tests::common::{create_test_store, TEST_ID};

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
    fn calculate_merkle_hash_for__empty_record() {
        let timestamp = 1_765_432_100_123_456_789;
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let mut element = Element::new(&Path::new("::root::node::leaf").unwrap());
        element.set_id(TEST_ID[0]);
        element.metadata.set_created_at(timestamp);
        element.metadata.updated_at = timestamp;

        let hash = interface.calculate_merkle_hash_for(&element).unwrap();
        assert_eq!(
            hex::encode(hash),
            "480f2ebbbccb3883d80bceebdcda48f1f8c1577e1f219b6213d61048f54473d6"
        );
    }

    #[test]
    fn calculate_merkle_hash_for__with_children() {
        let timestamp = 1_765_432_100_123_456_789;
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let mut element = Element::new(&Path::new("::root::node").unwrap());
        element.set_id(TEST_ID[0]);
        element.metadata.set_created_at(1_765_432_100_123_456_789);
        element.metadata.updated_at = timestamp;
        assert!(interface.save(element.id(), &mut element).unwrap());
        let mut child1 = Element::new(&Path::new("::root::node::leaf1").unwrap());
        let mut child2 = Element::new(&Path::new("::root::node::leaf2").unwrap());
        let mut child3 = Element::new(&Path::new("::root::node::leaf3").unwrap());
        child1.set_id(TEST_ID[1]);
        child2.set_id(TEST_ID[2]);
        child3.set_id(TEST_ID[3]);
        child1.metadata.set_created_at(timestamp);
        child2.metadata.set_created_at(timestamp);
        child3.metadata.set_created_at(timestamp);
        child1.metadata.updated_at = timestamp;
        child2.metadata.updated_at = timestamp;
        child3.metadata.updated_at = timestamp;
        assert!(interface.save(child1.id(), &mut child1).unwrap());
        assert!(interface.save(child2.id(), &mut child2).unwrap());
        assert!(interface.save(child3.id(), &mut child3).unwrap());
        element.child_ids = vec![child1.id(), child2.id(), child3.id()];
        assert!(interface.save(element.id(), &mut element).unwrap());

        assert_eq!(
            hex::encode(interface.calculate_merkle_hash_for(&child1).unwrap()),
            "807554028555897b6fa91f7538e48eee2d43720c5093c0a96ed393175ef95358",
        );
        assert_eq!(
            hex::encode(interface.calculate_merkle_hash_for(&child2).unwrap()),
            "d695f0f9ac154d051b4ea87ec7bab761538f41b4a8539921baa23d6540c6c09f",
        );
        assert_eq!(
            hex::encode(interface.calculate_merkle_hash_for(&child3).unwrap()),
            "7dca2754b8d6d95cdb08d37306191fdd72ba1a9d9a549c0d63d018dd98298a39",
        );
        assert_eq!(
            hex::encode(interface.calculate_merkle_hash_for(&element).unwrap()),
            "b145cd6657adc20fdcf410ff9a194cb28836ef7cfdd70ef6abdb69c689246418",
        );
    }

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
    fn test_save__update_merkle_hash() {
        // TODO: This is best done when there's data
        todo!()
    }

    #[test]
    #[ignore]
    fn test_save__update_merkle_hash_with_children() {
        // TODO: This is best done when there's data
        todo!()
    }

    #[test]
    #[ignore]
    fn test_save__update_merkle_hash_with_parents() {
        // TODO: This is best done when there's data
        todo!()
    }

    #[test]
    #[ignore]
    fn validate() {
        todo!()
    }
}
