use claims::{assert_none, assert_ok};

use super::*;
use crate::entities::{Data, Element};
use crate::tests::common::{create_test_store, EmptyData, Page, Paragraph, TEST_ID};

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
        let data = EmptyData {
            storage: element.clone(),
        };

        let hash = interface.calculate_merkle_hash_for(&data).unwrap();
        assert_eq!(
            hex::encode(hash),
            "aefc29aae587eb4a3bc44cb29dc30624db9936566aa2f0d0df1cd07793a94e6e"
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
        let mut page = Page::new_from_element("Node", element);
        assert!(interface.save(page.id(), &mut page).unwrap());
        let child1 = Element::new(&Path::new("::root::node::leaf1").unwrap());
        let child2 = Element::new(&Path::new("::root::node::leaf2").unwrap());
        let child3 = Element::new(&Path::new("::root::node::leaf3").unwrap());
        let mut para1 = Paragraph::new_from_element("Leaf1", child1);
        let mut para2 = Paragraph::new_from_element("Leaf2", child2);
        let mut para3 = Paragraph::new_from_element("Leaf3", child3);
        para1.element_mut().set_id(TEST_ID[1]);
        para2.element_mut().set_id(TEST_ID[2]);
        para3.element_mut().set_id(TEST_ID[3]);
        para1.element_mut().metadata.set_created_at(timestamp);
        para2.element_mut().metadata.set_created_at(timestamp);
        para3.element_mut().metadata.set_created_at(timestamp);
        para1.element_mut().metadata.updated_at = timestamp;
        para2.element_mut().metadata.updated_at = timestamp;
        para3.element_mut().metadata.updated_at = timestamp;
        assert!(interface.save(para1.id(), &mut para1).unwrap());
        assert!(interface.save(para2.id(), &mut para2).unwrap());
        assert!(interface.save(para3.id(), &mut para3).unwrap());
        page.element_mut().child_ids = vec![para1.id(), para2.id(), para3.id()];
        assert!(interface.save(page.id(), &mut page).unwrap());

        assert_eq!(
            hex::encode(interface.calculate_merkle_hash_for(&para1).unwrap()),
            "a93a438ee86aee5ae8b89207e41dff50aa4c7a2e4854bebb56399dd78cbb1b3c",
        );
        assert_eq!(
            hex::encode(interface.calculate_merkle_hash_for(&para2).unwrap()),
            "c3800c5d62756a45d03934b150526728a4e2d880274c04dda8a2c574847c8639",
        );
        assert_eq!(
            hex::encode(interface.calculate_merkle_hash_for(&para3).unwrap()),
            "ce9ec97a0f52e9a2ecfd0e7e5c35da4330746d22c373d65f9ff28c4931ea1210",
        );
        assert_eq!(
            hex::encode(interface.calculate_merkle_hash_for(&page).unwrap()),
            "9b042f16801a9acfe607b01e626f5642ce15d5da893d00a19d751f48caccc5e6",
        );
    }

    #[test]
    fn children_of() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let element = Element::new(&Path::new("::root::node").unwrap());
        let mut page = Page::new_from_element("Node", element);
        assert!(interface.save(page.id(), &mut page).unwrap());
        assert_eq!(interface.children_of(&page).unwrap(), vec![]);

        let child1 = Element::new(&Path::new("::root::node::leaf1").unwrap());
        let child2 = Element::new(&Path::new("::root::node::leaf2").unwrap());
        let child3 = Element::new(&Path::new("::root::node::leaf3").unwrap());
        let mut para1 = Paragraph::new_from_element("Leaf1", child1);
        let mut para2 = Paragraph::new_from_element("Leaf2", child2);
        let mut para3 = Paragraph::new_from_element("Leaf3", child3);
        assert!(interface.save(para1.id(), &mut para1).unwrap());
        assert!(interface.save(para2.id(), &mut para2).unwrap());
        assert!(interface.save(para3.id(), &mut para3).unwrap());
        page.element_mut().child_ids = vec![para1.id(), para2.id(), para3.id()];
        assert!(interface.save(page.id(), &mut page).unwrap());
        assert_eq!(
            interface.children_of(&page).unwrap(),
            vec![para1, para2, para3]
        );
    }

    #[test]
    fn find_by_id__existent() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let element = Element::new(&Path::new("::root::node::leaf").unwrap());
        let mut para = Paragraph::new_from_element("Leaf", element);
        let id = para.id();
        assert!(interface.save(id, &mut para).unwrap());

        assert_eq!(interface.find_by_id(id).unwrap(), Some(para));
    }

    #[test]
    fn find_by_id__non_existent() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);

        assert_none!(interface.find_by_id::<Page>(Id::new()).unwrap());
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
        let mut para = Paragraph::new_from_element("Leaf", element);

        assert_ok!(interface.save(para.id(), &mut para));
    }

    #[test]
    fn test_save__multiple() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let element1 = Element::new(&Path::new("::root::node1").unwrap());
        let element2 = Element::new(&Path::new("::root::node2").unwrap());
        let mut page1 = Page::new_from_element("Node1", element1);
        let mut page2 = Page::new_from_element("Node2", element2);

        assert!(interface.save(page1.id(), &mut page1).unwrap());
        assert!(interface.save(page2.id(), &mut page2).unwrap());
        assert_eq!(interface.find_by_id(page1.id()).unwrap(), Some(page1));
        assert_eq!(interface.find_by_id(page2.id()).unwrap(), Some(page2));
    }

    #[test]
    fn test_save__not_dirty() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let element = Element::new(&Path::new("::root::node::leaf").unwrap());
        let mut para = Paragraph::new_from_element("Leaf", element);
        let id = para.id();

        assert!(interface.save(id, &mut para).unwrap());
        para.element_mut().update();
        assert!(interface.save(id, &mut para).unwrap());
    }

    #[test]
    fn test_save__too_old() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let element1 = Element::new(&Path::new("::root::node::leaf").unwrap());
        let mut para1 = Paragraph::new_from_element("Leaf", element1);
        let mut para2 = para1.clone();
        let id = para1.id();

        assert!(interface.save(id, &mut para1).unwrap());
        para1.element_mut().update();
        para2.element_mut().update();
        assert!(interface.save(id, &mut para1).unwrap());
        assert!(!interface.save(id, &mut para2).unwrap());
    }

    #[test]
    fn test_save__update_existing() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let element = Element::new(&Path::new("::root::node::leaf").unwrap());
        let mut para = Paragraph::new_from_element("Leaf", element);
        let id = para.id();
        assert!(interface.save(id, &mut para).unwrap());

        // TODO: Modify the element's data and check it changed

        assert!(interface.save(id, &mut para).unwrap());
        assert_eq!(interface.find_by_id(id).unwrap(), Some(para));
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
