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
    fn find_by_id_raw() {
        todo!()
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
    fn save__basic() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let element = Element::new(&Path::new("::root::node::leaf").unwrap());
        let mut para = Paragraph::new_from_element("Leaf", element);

        assert_ok!(interface.save(para.id(), &mut para));
    }

    #[test]
    fn save__multiple() {
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
    fn save__not_dirty() {
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
    fn save__too_old() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let element1 = Element::new(&Path::new("::root::node::leaf").unwrap());
        let mut para1 = Paragraph::new_from_element("Leaf", element1);
        let mut para2 = para1.clone();
        let id = para1.id();

        assert!(interface.save(id, &mut para1).unwrap());
        para2.element_mut().update();
        sleep(Duration::from_millis(1));
        para1.element_mut().update();
        assert!(interface.save(id, &mut para1).unwrap());
        assert!(!interface.save(id, &mut para2).unwrap());
    }

    #[test]
    fn save__update_existing() {
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
    fn save__update_merkle_hash() {
        // TODO: This is best done when there's data
        todo!()
    }

    #[test]
    #[ignore]
    fn save__update_merkle_hash_with_children() {
        // TODO: This is best done when there's data
        todo!()
    }

    #[test]
    #[ignore]
    fn save__update_merkle_hash_with_parents() {
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
mod interface__apply_actions {
    use super::*;

    #[test]
    fn apply_action__add() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let page = Page::new_from_element("Test Page", Element::new(&Path::new("::test").unwrap()));
        let serialized = to_vec(&page).unwrap();
        let action = Action::Add(page.id(), serialized);

        assert!(interface.apply_action::<Page>(action).is_ok());

        // Verify the page was added
        let retrieved_page = interface.find_by_id::<Page>(page.id()).unwrap();
        assert!(retrieved_page.is_some());
        assert_eq!(retrieved_page.unwrap().title, "Test Page");
    }

    #[test]
    fn apply_action__update() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let mut page =
            Page::new_from_element("Old Title", Element::new(&Path::new("::test").unwrap()));
        assert!(interface.save(page.id(), &mut page).unwrap());

        page.title = "New Title".to_owned();
        page.element_mut().update();
        let serialized = to_vec(&page).unwrap();
        let action = Action::Update(page.id(), serialized);

        assert!(interface.apply_action::<Page>(action).is_ok());

        // Verify the page was updated
        let retrieved_page = interface.find_by_id::<Page>(page.id()).unwrap().unwrap();
        assert_eq!(retrieved_page.title, "New Title");
    }

    #[test]
    fn apply_action__delete() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let mut page =
            Page::new_from_element("Test Page", Element::new(&Path::new("::test").unwrap()));
        assert!(interface.save(page.id(), &mut page).unwrap());

        let action = Action::Delete(page.id());

        assert!(interface.apply_action::<Page>(action).is_ok());

        // Verify the page was deleted
        let retrieved_page = interface.find_by_id::<Page>(page.id()).unwrap();
        assert!(retrieved_page.is_none());
    }

    #[test]
    fn apply_action__compare() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let page = Page::new_from_element("Test Page", Element::new(&Path::new("::test").unwrap()));
        let action = Action::Compare(page.id());

        // Compare should fail
        assert!(interface.apply_action::<Page>(action).is_err());
    }

    #[test]
    fn apply_action__wrong_type() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let page = Page::new_from_element("Test Page", Element::new(&Path::new("::test").unwrap()));
        let serialized = to_vec(&page).unwrap();
        let action = Action::Add(page.id(), serialized);

        // Trying to apply a Page action as if it were a Paragraph should fail
        assert!(interface.apply_action::<Paragraph>(action).is_err());
    }

    #[test]
    fn apply_action__non_existent_update() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let page = Page::new_from_element("Test Page", Element::new(&Path::new("::test").unwrap()));
        let serialized = to_vec(&page).unwrap();
        let action = Action::Update(page.id(), serialized);

        // Updating a non-existent page should still succeed (it will be added)
        assert!(interface.apply_action::<Page>(action).is_ok());

        // Verify the page was added
        let retrieved_page = interface.find_by_id::<Page>(page.id()).unwrap();
        assert!(retrieved_page.is_some());
        assert_eq!(retrieved_page.unwrap().title, "Test Page");
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
        foreign.element_mut().merkle_hash = interface
            .calculate_merkle_hash_for(&foreign, false)
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
        foreign.element_mut().merkle_hash = interface
            .calculate_merkle_hash_for(&foreign, false)
            .unwrap();

        let result = interface.compare_trees(&foreign).unwrap();
        assert_eq!(
            result,
            (
                vec![],
                vec![Action::Update(local.id(), to_vec(&local).unwrap())]
            )
        );
    }

    #[test]
    fn compare_trees__foreign_newer() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let element = Element::new(&Path::new("::root::node").unwrap());
        let mut local = Page::new_from_element("Old Test Page", element.clone());
        let mut foreign = Page::new_from_element("Test Page", element);

        assert!(interface.save(local.id(), &mut local).unwrap());
        foreign.element_mut().merkle_hash = interface
            .calculate_merkle_hash_for(&foreign, false)
            .unwrap();

        // Make foreign newer
        sleep(Duration::from_millis(10));
        foreign.element_mut().update();

        let result = interface.compare_trees(&foreign).unwrap();
        assert_eq!(
            result,
            (
                vec![Action::Update(foreign.id(), to_vec(&foreign).unwrap())],
                vec![]
            )
        );
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
        foreign_page.element_mut().merkle_hash = interface
            .calculate_merkle_hash_for(&foreign_page, false)
            .unwrap();
        foreign_para1.element_mut().merkle_hash = interface
            .calculate_merkle_hash_for(&foreign_para1, false)
            .unwrap();
        foreign_para3.element_mut().merkle_hash = interface
            .calculate_merkle_hash_for(&foreign_para3, false)
            .unwrap();

        let (local_actions, foreign_actions) = interface.compare_trees(&foreign_page).unwrap();

        assert_eq!(
            local_actions,
            vec![
                // Page needs update due to different child structure
                Action::Update(foreign_page.id(), to_vec(&foreign_page).unwrap()),
                // Para1 needs comparison due to different hash
                Action::Compare(local_para1.id()),
            ]
        );
        local_para2.element_mut().is_dirty = true;
        assert_eq!(
            foreign_actions,
            vec![
                // Para1 needs comparison due to different hash
                Action::Compare(local_para1.id()),
                // Para2 needs to be added to foreign
                Action::Add(local_para2.id(), to_vec(&local_para2).unwrap()),
                // Para3 needs to be added locally, but we don't have the data, so we compare
                Action::Compare(foreign_para3.id()),
            ]
        );

        // Compare the updated para1
        let (local_para1_actions, foreign_para1_actions) =
            interface.compare_trees(&foreign_para1).unwrap();

        assert_eq!(
            local_para1_actions,
            vec![Action::Update(
                foreign_para1.id(),
                to_vec(&foreign_para1).unwrap()
            )]
        );
        assert_eq!(foreign_para1_actions, vec![]);

        // Compare para3 which doesn't exist locally
        let (local_para3_actions, foreign_para3_actions) =
            interface.compare_trees(&foreign_para3).unwrap();

        assert_eq!(
            local_para3_actions,
            vec![Action::Add(
                foreign_para3.id(),
                to_vec(&foreign_para3).unwrap()
            )]
        );
        assert_eq!(foreign_para3_actions, vec![]);
    }
}

#[cfg(test)]
mod hashing {
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
    fn calculate_merkle_hash_for__cached_values() {
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
            ChildInfo::new(para1.id(), para1.element().merkle_hash()),
            ChildInfo::new(para2.id(), para2.element().merkle_hash()),
            ChildInfo::new(para3.id(), para3.element().merkle_hash()),
        ];
        assert!(interface.save(page.id(), &mut page).unwrap());

        let mut hasher0 = Sha256::new();
        hasher0.update(page.id().as_bytes());
        hasher0.update(&to_vec(&page.title).unwrap());
        hasher0.update(&to_vec(&page.element().metadata()).unwrap());
        let expected_hash0: [u8; 32] = hasher0.finalize().into();

        let mut hasher1 = Sha256::new();
        hasher1.update(para1.id().as_bytes());
        hasher1.update(&to_vec(&para1.text).unwrap());
        hasher1.update(&to_vec(&para1.element().metadata()).unwrap());
        let expected_hash1: [u8; 32] = hasher1.finalize().into();
        let mut hasher1b = Sha256::new();
        hasher1b.update(expected_hash1);
        let expected_hash1b: [u8; 32] = hasher1b.finalize().into();

        let mut hasher2 = Sha256::new();
        hasher2.update(para2.id().as_bytes());
        hasher2.update(&to_vec(&para2.text).unwrap());
        hasher2.update(&to_vec(&para2.element().metadata()).unwrap());
        let expected_hash2: [u8; 32] = hasher2.finalize().into();
        let mut hasher2b = Sha256::new();
        hasher2b.update(expected_hash2);
        let expected_hash2b: [u8; 32] = hasher2b.finalize().into();

        let mut hasher3 = Sha256::new();
        hasher3.update(para3.id().as_bytes());
        hasher3.update(&to_vec(&para3.text).unwrap());
        hasher3.update(&to_vec(&para3.element().metadata()).unwrap());
        let expected_hash3: [u8; 32] = hasher3.finalize().into();
        let mut hasher3b = Sha256::new();
        hasher3b.update(expected_hash3);
        let expected_hash3b: [u8; 32] = hasher3b.finalize().into();

        let mut hasher = Sha256::new();
        hasher.update(&expected_hash0);
        hasher.update(&expected_hash1b);
        hasher.update(&expected_hash2b);
        hasher.update(&expected_hash3b);
        let expected_hash: [u8; 32] = hasher.finalize().into();

        assert_eq!(page.calculate_merkle_hash().unwrap(), expected_hash0);
        assert_eq!(
            interface.calculate_merkle_hash_for(&para1, false).unwrap(),
            expected_hash1b
        );
        assert_eq!(
            interface.calculate_merkle_hash_for(&para2, false).unwrap(),
            expected_hash2b
        );
        assert_eq!(
            interface.calculate_merkle_hash_for(&para3, false).unwrap(),
            expected_hash3b
        );
        assert_eq!(
            interface.calculate_merkle_hash_for(&page, false).unwrap(),
            expected_hash
        );
    }

    #[test]
    #[ignore]
    fn calculate_merkle_hash_for__recalculated_values() {
        // TODO: Later, tests should be added for recalculating the hashes, and
        // TODO: especially checking when the data has been interfered with or
        // TODO: otherwise arrived at an invalid state.
        todo!()
    }
}
