use std::thread::sleep;
use std::time::Duration;

use calimero_test_utils::storage::create_test_store;
use claims::{assert_none, assert_ok};

use super::*;
use crate::entities::{ChildInfo, Data, Element};
use crate::tests::common::{EmptyData, Page, Paragraph, TEST_ID};

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

        let hash = interface.calculate_merkle_hash_for(&data, false).unwrap();
        assert_eq!(
            hex::encode(hash),
            "173f9a17aa3c6acdad8cdaf06cea4aa4eb7c87cb99e07507a417d6588e679607"
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
        page.paragraphs.child_info = vec![
            ChildInfo::new(para1.id(), para1.element().merkle_hash()),
            ChildInfo::new(para2.id(), para2.element().merkle_hash()),
            ChildInfo::new(para3.id(), para3.element().merkle_hash()),
        ];
        assert!(interface.save(page.id(), &mut page).unwrap());

        assert_eq!(
            hex::encode(interface.calculate_merkle_hash_for(&para1, false).unwrap()),
            "9c4d6363cca5bdb5829f0aa832b573d6befd26227a0e2c3cc602edd9fda88db1",
        );
        assert_eq!(
            hex::encode(interface.calculate_merkle_hash_for(&para2, false).unwrap()),
            "449f30903c94a488f1767b91bc6626fafd82189130cf41e427f96df19a727d8b",
        );
        assert_eq!(
            hex::encode(interface.calculate_merkle_hash_for(&para3, false).unwrap()),
            "43098decf78bf10dc4c31191a5f59d277ae524859583e48689482c9ba85c5f61",
        );
        assert_eq!(
            hex::encode(interface.calculate_merkle_hash_for(&page, false).unwrap()),
            "7593806c462bfadd97ed5228a3a60e492cce4b725f2c0e72e6e5b0f7996ee394",
        );
    }

    #[test]
    fn children_of() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let element = Element::new(&Path::new("::root::node").unwrap());
        let mut page = Page::new_from_element("Node", element);
        assert!(interface.save(page.id(), &mut page).unwrap());
        assert_eq!(interface.children_of(&page.paragraphs).unwrap(), vec![]);

        let child1 = Element::new(&Path::new("::root::node::leaf1").unwrap());
        let child2 = Element::new(&Path::new("::root::node::leaf2").unwrap());
        let child3 = Element::new(&Path::new("::root::node::leaf3").unwrap());
        let mut para1 = Paragraph::new_from_element("Leaf1", child1);
        let mut para2 = Paragraph::new_from_element("Leaf2", child2);
        let mut para3 = Paragraph::new_from_element("Leaf3", child3);
        assert!(interface.save(para1.id(), &mut para1).unwrap());
        assert!(interface.save(para2.id(), &mut para2).unwrap());
        assert!(interface.save(para3.id(), &mut para3).unwrap());
        page.paragraphs.child_info = vec![
            ChildInfo::new(para1.id(), para1.calculate_merkle_hash().unwrap()),
            ChildInfo::new(para2.id(), para2.calculate_merkle_hash().unwrap()),
            ChildInfo::new(para3.id(), para3.calculate_merkle_hash().unwrap()),
        ];
        assert!(interface.save(page.id(), &mut page).unwrap());
        assert_eq!(
            interface.children_of(&page.paragraphs).unwrap(),
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

#[cfg(test)]
mod interface__comparison {
    use super::*;

    #[test]
    fn compare_trees__identical() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let element = Element::new(&Path::new("::root::node").unwrap());
        let mut local = Page::new_from_element("Test Page", element);
        let mut foreign = local.clone();

        assert!(interface.save(local.id(), &mut local).unwrap());
        foreign.element_mut().merkle_hash = foreign
            .calculate_full_merkle_hash(&interface, false)
            .unwrap();
        assert_eq!(
            local.element().merkle_hash(),
            foreign.element().merkle_hash()
        );

        let result = interface.compare_trees(&foreign).unwrap();
        assert_eq!(result, (vec![], vec![]));
    }

    #[test]
    fn compare_trees__local_newer() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let element = Element::new(&Path::new("::root::node").unwrap());
        let mut local = Page::new_from_element("Test Page", element.clone());
        let mut foreign = Page::new_from_element("Old Test Page", element);

        // Make local newer
        sleep(Duration::from_millis(10));
        local.element_mut().update();

        assert!(interface.save(local.id(), &mut local).unwrap());
        foreign.element_mut().merkle_hash = foreign
            .calculate_full_merkle_hash(&interface, false)
            .unwrap();

        let result = interface.compare_trees(&foreign).unwrap();
        assert_eq!(result, (vec![], vec![Action::Update(local.id())]));
    }

    #[test]
    fn compare_trees__foreign_newer() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let element = Element::new(&Path::new("::root::node").unwrap());
        let mut local = Page::new_from_element("Old Test Page", element.clone());
        let mut foreign = Page::new_from_element("Test Page", element);

        assert!(interface.save(local.id(), &mut local).unwrap());
        foreign.element_mut().merkle_hash = foreign
            .calculate_full_merkle_hash(&interface, false)
            .unwrap();

        // Make foreign newer
        sleep(Duration::from_millis(10));
        foreign.element_mut().update();

        let result = interface.compare_trees(&foreign).unwrap();
        assert_eq!(result, (vec![Action::Update(local.id())], vec![]));
    }

    #[test]
    fn compare_trees__with_collections() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);

        let page_element = Element::new(&Path::new("::root::node").unwrap());
        let para1_element = Element::new(&Path::new("::root::node::leaf1").unwrap());
        let para2_element = Element::new(&Path::new("::root::node::leaf2").unwrap());
        let para3_element = Element::new(&Path::new("::root::node::leaf3").unwrap());

        let mut local_page = Page::new_from_element("Local Page", page_element.clone());
        let mut local_para1 =
            Paragraph::new_from_element("Local Paragraph 1", para1_element.clone());
        let mut local_para2 = Paragraph::new_from_element("Local Paragraph 2", para2_element);

        let mut foreign_page = Page::new_from_element("Foreign Page", page_element);
        let mut foreign_para1 = Paragraph::new_from_element("Updated Paragraph 1", para1_element);
        let mut foreign_para3 = Paragraph::new_from_element("Foreign Paragraph 3", para3_element);

        local_page.paragraphs.child_info = vec![
            ChildInfo::new(
                local_para1.id(),
                local_para1.calculate_merkle_hash().unwrap(),
            ),
            ChildInfo::new(
                local_para2.id(),
                local_para2.calculate_merkle_hash().unwrap(),
            ),
        ];

        foreign_page.paragraphs.child_info = vec![
            ChildInfo::new(
                local_para1.id(),
                foreign_para1.calculate_merkle_hash().unwrap(),
            ),
            ChildInfo::new(
                foreign_para3.id(),
                foreign_para3.calculate_merkle_hash().unwrap(),
            ),
        ];

        assert!(interface.save(local_page.id(), &mut local_page).unwrap());
        assert!(interface.save(local_para1.id(), &mut local_para1).unwrap());
        assert!(interface.save(local_para2.id(), &mut local_para2).unwrap());
        foreign_page.element_mut().merkle_hash = foreign_page
            .calculate_full_merkle_hash(&interface, false)
            .unwrap();
        foreign_para1.element_mut().merkle_hash = foreign_para1
            .calculate_full_merkle_hash(&interface, false)
            .unwrap();
        foreign_para3.element_mut().merkle_hash = foreign_para3
            .calculate_full_merkle_hash(&interface, false)
            .unwrap();

        let (local_actions, mut foreign_actions) = interface.compare_trees(&foreign_page).unwrap();
        foreign_actions.sort();

        assert_eq!(
            local_actions,
            vec![
                Action::Update(local_page.id()), // Page needs update due to different child structure
                Action::Compare(local_para1.id()), // Para1 needs comparison due to different hash
                Action::Add(foreign_para3.id()), // Para3 needs to be added locally
            ]
        );
        assert_eq!(
            foreign_actions,
            vec![
                Action::Add(local_para2.id()),     // Para2 needs to be added to foreign
                Action::Compare(local_para1.id()), // Para1 needs comparison due to different hash
            ]
        );

        // Compare the updated para1
        let (local_para1_actions, foreign_para1_actions) =
            interface.compare_trees(&foreign_para1).unwrap();

        assert_eq!(local_para1_actions, vec![Action::Update(local_para1.id())]);
        assert_eq!(foreign_para1_actions, vec![]);

        // Compare para3 which doesn't exist locally
        let (local_para3_actions, foreign_para3_actions) =
            interface.compare_trees(&foreign_para3).unwrap();

        assert_eq!(local_para3_actions, vec![Action::Add(foreign_para3.id())]);
        assert_eq!(foreign_para3_actions, vec![]);
    }
}
