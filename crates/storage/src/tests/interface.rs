use std::thread::sleep;
use std::time::Duration;

use claims::{assert_none, assert_ok};

use super::*;
use crate::entities::{Data, Element};
use crate::tests::common::{Page, Paragraph};

#[cfg(test)]
mod interface__public_methods {
    use super::*;

    #[test]
    fn children_of() {
        let element = Element::new(&Path::new("::root::node").unwrap());
        let mut page = Page::new_from_element("Node", element);
        assert!(Interface::save(&mut page).unwrap());
        assert_eq!(
            Interface::children_of(page.id(), &page.paragraphs).unwrap(),
            vec![]
        );

        let child1 = Element::new(&Path::new("::root::node::leaf1").unwrap());
        let child2 = Element::new(&Path::new("::root::node::leaf2").unwrap());
        let child3 = Element::new(&Path::new("::root::node::leaf3").unwrap());
        let mut para1 = Paragraph::new_from_element("Leaf1", child1);
        let mut para2 = Paragraph::new_from_element("Leaf2", child2);
        let mut para3 = Paragraph::new_from_element("Leaf3", child3);
        assert!(Interface::save(&mut page).unwrap());
        assert!(Interface::add_child_to(page.id(), &mut page.paragraphs, &mut para1).unwrap());
        assert!(Interface::add_child_to(page.id(), &mut page.paragraphs, &mut para2).unwrap());
        assert!(Interface::add_child_to(page.id(), &mut page.paragraphs, &mut para3).unwrap());
        assert_eq!(
            Interface::children_of(page.id(), &page.paragraphs).unwrap(),
            vec![para1, para2, para3]
        );
    }

    #[test]
    fn find_by_id__existent() {
        let element = Element::new(&Path::new("::root::node").unwrap());
        let mut page = Page::new_from_element("Leaf", element);
        let id = page.id();
        assert!(Interface::save(&mut page).unwrap());

        assert_eq!(Interface::find_by_id(id).unwrap(), Some(page));
    }

    #[test]
    fn find_by_id__non_existent() {
        assert_none!(Interface::find_by_id::<Page>(Id::new()).unwrap());
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
        let element = Element::new(&Path::new("::root::node").unwrap());
        let mut page = Page::new_from_element("Node", element);

        assert_ok!(Interface::save(&mut page));
    }

    #[test]
    fn save__multiple() {
        let element1 = Element::new(&Path::new("::root::node1").unwrap());
        let element2 = Element::new(&Path::new("::root::node2").unwrap());
        let mut page1 = Page::new_from_element("Node1", element1);
        let mut page2 = Page::new_from_element("Node2", element2);

        assert!(Interface::save(&mut page1).unwrap());
        assert!(Interface::save(&mut page2).unwrap());
        assert_eq!(Interface::find_by_id(page1.id()).unwrap(), Some(page1));
        assert_eq!(Interface::find_by_id(page2.id()).unwrap(), Some(page2));
    }

    #[test]
    fn save__not_dirty() {
        let element = Element::new(&Path::new("::root::node").unwrap());
        let mut page = Page::new_from_element("Node", element);

        assert!(Interface::save(&mut page).unwrap());
        page.element_mut().update();
        assert!(Interface::save(&mut page).unwrap());
    }

    #[test]
    fn save__too_old() {
        let element1 = Element::new(&Path::new("::root::node").unwrap());
        let mut page1 = Page::new_from_element("Node", element1);
        let mut page2 = page1.clone();

        assert!(Interface::save(&mut page1).unwrap());
        page2.element_mut().update();
        sleep(Duration::from_millis(1));
        page1.element_mut().update();
        assert!(Interface::save(&mut page1).unwrap());
        assert!(!Interface::save(&mut page2).unwrap());
    }

    #[test]
    fn save__update_existing() {
        let element = Element::new(&Path::new("::root::node").unwrap());
        let mut page = Page::new_from_element("Node", element);
        let id = page.id();
        assert!(Interface::save(&mut page).unwrap());

        // TODO: Modify the element's data and check it changed

        assert!(Interface::save(&mut page).unwrap());
        assert_eq!(Interface::find_by_id(id).unwrap(), Some(page));
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
        let page = Page::new_from_element("Test Page", Element::new(&Path::new("::test").unwrap()));
        let serialized = to_vec(&page).unwrap();
        let action = Action::Add {
            id: page.id(),
            type_id: 102,
            data: serialized,
            ancestors: vec![],
        };

        assert!(Interface::apply_action::<Page>(action).is_ok());

        // Verify the page was added
        let retrieved_page = Interface::find_by_id::<Page>(page.id()).unwrap();
        assert!(retrieved_page.is_some());
        assert_eq!(retrieved_page.unwrap().title, "Test Page");
    }

    #[test]
    fn apply_action__update() {
        let mut page =
            Page::new_from_element("Old Title", Element::new(&Path::new("::test").unwrap()));
        assert!(Interface::save(&mut page).unwrap());

        page.title = "New Title".to_owned();
        page.element_mut().update();
        let serialized = to_vec(&page).unwrap();
        let action = Action::Update {
            id: page.id(),
            type_id: 102,
            data: serialized,
            ancestors: vec![],
        };

        assert!(Interface::apply_action::<Page>(action).is_ok());

        // Verify the page was updated
        let retrieved_page = Interface::find_by_id::<Page>(page.id()).unwrap().unwrap();
        assert_eq!(retrieved_page.title, "New Title");
    }

    #[test]
    fn apply_action__delete() {
        let mut page =
            Page::new_from_element("Test Page", Element::new(&Path::new("::test").unwrap()));
        assert!(Interface::save(&mut page).unwrap());

        let action = Action::Delete {
            id: page.id(),
            ancestors: vec![],
        };

        assert!(Interface::apply_action::<Page>(action).is_ok());

        // Verify the page was deleted
        let retrieved_page = Interface::find_by_id::<Page>(page.id()).unwrap();
        assert!(retrieved_page.is_none());
    }

    #[test]
    fn apply_action__compare() {
        let page = Page::new_from_element("Test Page", Element::new(&Path::new("::test").unwrap()));
        let action = Action::Compare { id: page.id() };

        // Compare should fail
        assert!(Interface::apply_action::<Page>(action).is_err());
    }

    #[test]
    fn apply_action__wrong_type() {
        let page = Page::new_from_element("Test Page", Element::new(&Path::new("::test").unwrap()));
        let serialized = to_vec(&page).unwrap();
        let action = Action::Add {
            id: page.id(),
            type_id: 102,
            data: serialized,
            ancestors: vec![],
        };

        // Trying to apply a Page action as if it were a Paragraph should fail
        assert!(Interface::apply_action::<Paragraph>(action).is_err());
    }

    #[test]
    fn apply_action__non_existent_update() {
        let page = Page::new_from_element("Test Page", Element::new(&Path::new("::test").unwrap()));
        let serialized = to_vec(&page).unwrap();
        let action = Action::Update {
            id: page.id(),
            type_id: 102,
            data: serialized,
            ancestors: vec![],
        };

        // Updating a non-existent page should still succeed (it will be added)
        assert!(Interface::apply_action::<Page>(action).is_ok());

        // Verify the page was added
        let retrieved_page = Interface::find_by_id::<Page>(page.id()).unwrap();
        assert!(retrieved_page.is_some());
        assert_eq!(retrieved_page.unwrap().title, "Test Page");
    }
}
