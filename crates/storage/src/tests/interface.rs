use std::thread::sleep;
use std::time::Duration;

use claims::{assert_none, assert_ok};

use super::*;
use crate::entities::{Data, Element};
use crate::mocks::{ForeignInterface, MockVM};
use crate::tests::common::{Page, Paragraph};

#[cfg(test)]
mod interface__public_methods {
    use super::*;

    #[test]
    fn children_of() {
        let element = Element::new::<MockVM>(&Path::new("::root::node").unwrap());
        let mut page = Page::new_from_element("Node", element);
        assert!(Interface::save(&mut page).unwrap());
        assert_eq!(
            Interface::children_of(page.id(), &page.paragraphs).unwrap(),
            vec![]
        );

        let child1 = Element::new::<MockVM>(&Path::new("::root::node::leaf1").unwrap());
        let child2 = Element::new::<MockVM>(&Path::new("::root::node::leaf2").unwrap());
        let child3 = Element::new::<MockVM>(&Path::new("::root::node::leaf3").unwrap());
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
        let element = Element::new::<MockVM>(&Path::new("::root::node").unwrap());
        let mut page = Page::new_from_element("Leaf", element);
        let id = page.id();
        assert!(Interface::save(&mut page).unwrap());

        assert_eq!(Interface::find_by_id(id).unwrap(), Some(page));
    }

    #[test]
    fn find_by_id__non_existent() {
        assert_none!(Interface::find_by_id::<Page>(Id::new::<MockVM>()).unwrap());
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
        let element = Element::new::<MockVM>(&Path::new("::root::node").unwrap());
        let mut page = Page::new_from_element("Node", element);

        assert_ok!(Interface::save(&mut page));
    }

    #[test]
    fn save__multiple() {
        let element1 = Element::new::<MockVM>(&Path::new("::root::node1").unwrap());
        let element2 = Element::new::<MockVM>(&Path::new("::root::node2").unwrap());
        let mut page1 = Page::new_from_element("Node1", element1);
        let mut page2 = Page::new_from_element("Node2", element2);

        assert!(Interface::save(&mut page1).unwrap());
        assert!(Interface::save(&mut page2).unwrap());
        assert_eq!(Interface::find_by_id(page1.id()).unwrap(), Some(page1));
        assert_eq!(Interface::find_by_id(page2.id()).unwrap(), Some(page2));
    }

    #[test]
    fn save__not_dirty() {
        let element = Element::new::<MockVM>(&Path::new("::root::node").unwrap());
        let mut page = Page::new_from_element("Node", element);

        assert!(Interface::save(&mut page).unwrap());
        page.element_mut().update::<MockVM>();
        assert!(Interface::save(&mut page).unwrap());
    }

    #[test]
    fn save__too_old() {
        let element1 = Element::new::<MockVM>(&Path::new("::root::node").unwrap());
        let mut page1 = Page::new_from_element("Node", element1);
        let mut page2 = page1.clone();

        assert!(Interface::save(&mut page1).unwrap());
        page2.element_mut().update::<MockVM>();
        sleep(Duration::from_millis(1));
        page1.element_mut().update::<MockVM>();
        assert!(Interface::save(&mut page1).unwrap());
        assert!(!Interface::save(&mut page2).unwrap());
    }

    #[test]
    fn save__update_existing() {
        let element = Element::new::<MockVM>(&Path::new("::root::node").unwrap());
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
        let page = Page::new_from_element(
            "Test Page",
            Element::new::<MockVM>(&Path::new("::test").unwrap()),
        );
        let serialized = to_vec(&page).unwrap();
        let action = Action::Add(page.id(), serialized);

        assert!(Interface::apply_action::<Page>(action).is_ok());

        // Verify the page was added
        let retrieved_page = Interface::find_by_id::<Page>(page.id()).unwrap();
        assert!(retrieved_page.is_some());
        assert_eq!(retrieved_page.unwrap().title, "Test Page");
    }

    #[test]
    fn apply_action__update() {
        let mut page = Page::new_from_element(
            "Old Title",
            Element::new::<MockVM>(&Path::new("::test").unwrap()),
        );
        assert!(Interface::save(&mut page).unwrap());

        page.title = "New Title".to_owned();
        page.element_mut().update::<MockVM>();
        let serialized = to_vec(&page).unwrap();
        let action = Action::Update(page.id(), serialized);

        assert!(Interface::apply_action::<Page>(action).is_ok());

        // Verify the page was updated
        let retrieved_page = Interface::find_by_id::<Page>(page.id()).unwrap().unwrap();
        assert_eq!(retrieved_page.title, "New Title");
    }

    #[test]
    fn apply_action__delete() {
        let mut page = Page::new_from_element(
            "Test Page",
            Element::new::<MockVM>(&Path::new("::test").unwrap()),
        );
        assert!(Interface::save(&mut page).unwrap());

        let action = Action::Delete(page.id());

        assert!(Interface::apply_action::<Page>(action).is_ok());

        // Verify the page was deleted
        let retrieved_page = Interface::find_by_id::<Page>(page.id()).unwrap();
        assert!(retrieved_page.is_none());
    }

    #[test]
    fn apply_action__compare() {
        let page = Page::new_from_element(
            "Test Page",
            Element::new::<MockVM>(&Path::new("::test").unwrap()),
        );
        let action = Action::Compare(page.id());

        // Compare should fail
        assert!(Interface::apply_action::<Page>(action).is_err());
    }

    #[test]
    fn apply_action__wrong_type() {
        let page = Page::new_from_element(
            "Test Page",
            Element::new::<MockVM>(&Path::new("::test").unwrap()),
        );
        let serialized = to_vec(&page).unwrap();
        let action = Action::Add(page.id(), serialized);

        // Trying to apply a Page action as if it were a Paragraph should fail
        assert!(Interface::apply_action::<Paragraph>(action).is_err());
    }

    #[test]
    fn apply_action__non_existent_update() {
        let page = Page::new_from_element(
            "Test Page",
            Element::new::<MockVM>(&Path::new("::test").unwrap()),
        );
        let serialized = to_vec(&page).unwrap();
        let action = Action::Update(page.id(), serialized);

        // Updating a non-existent page should still succeed (it will be added)
        assert!(Interface::apply_action::<Page>(action).is_ok());

        // Verify the page was added
        let retrieved_page = Interface::find_by_id::<Page>(page.id()).unwrap();
        assert!(retrieved_page.is_some());
        assert_eq!(retrieved_page.unwrap().title, "Test Page");
    }
}

#[cfg(test)]
mod interface__comparison {
    use super::*;

    #[test]
    fn compare_trees__identical() {
        let element = Element::new::<MockVM>(&Path::new("::root::node").unwrap());
        let mut local = Page::new_from_element("Test Page", element);
        let mut foreign = local.clone();

        assert!(Interface::save(&mut local).unwrap());
        assert!(ForeignInterface::save(&mut foreign).unwrap());
        assert_eq!(
            local.element().merkle_hash(),
            foreign.element().merkle_hash()
        );

        let result = Interface::compare_trees(
            &foreign,
            &ForeignInterface::generate_comparison_data(&foreign).unwrap(),
        )
        .unwrap();
        assert_eq!(result, (vec![], vec![]));
    }

    #[test]
    fn compare_trees__local_newer() {
        let element = Element::new::<MockVM>(&Path::new("::root::node").unwrap());
        let mut local = Page::new_from_element("Test Page", element.clone());
        let mut foreign = Page::new_from_element("Old Test Page", element);

        assert!(ForeignInterface::save(&mut foreign).unwrap());

        // Make local newer
        sleep(Duration::from_millis(10));
        local.element_mut().update::<MockVM>();
        assert!(Interface::save(&mut local).unwrap());

        let result = Interface::compare_trees(
            &foreign,
            &ForeignInterface::generate_comparison_data(&foreign).unwrap(),
        )
        .unwrap();
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
        let element = Element::new::<MockVM>(&Path::new("::root::node").unwrap());
        let mut local = Page::new_from_element("Old Test Page", element.clone());
        let mut foreign = Page::new_from_element("Test Page", element);

        assert!(Interface::save(&mut local).unwrap());

        // Make foreign newer
        sleep(Duration::from_millis(10));
        foreign.element_mut().update::<MockVM>();
        assert!(ForeignInterface::save(&mut foreign).unwrap());

        let result = Interface::compare_trees(
            &foreign,
            &ForeignInterface::generate_comparison_data(&foreign).unwrap(),
        )
        .unwrap();
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
        let page_element = Element::new::<MockVM>(&Path::new("::root::node").unwrap());
        let para1_element = Element::new::<MockVM>(&Path::new("::root::node::leaf1").unwrap());
        let para2_element = Element::new::<MockVM>(&Path::new("::root::node::leaf2").unwrap());
        let para3_element = Element::new::<MockVM>(&Path::new("::root::node::leaf3").unwrap());

        let mut local_page = Page::new_from_element("Local Page", page_element.clone());
        let mut local_para1 =
            Paragraph::new_from_element("Local Paragraph 1", para1_element.clone());
        let mut local_para2 = Paragraph::new_from_element("Local Paragraph 2", para2_element);

        let mut foreign_page = Page::new_from_element("Foreign Page", page_element);
        let mut foreign_para1 = Paragraph::new_from_element("Updated Paragraph 1", para1_element);
        let mut foreign_para3 = Paragraph::new_from_element("Foreign Paragraph 3", para3_element);

        assert!(Interface::save(&mut local_page).unwrap());
        assert!(Interface::add_child_to(
            local_page.id(),
            &mut local_page.paragraphs,
            &mut local_para1
        )
        .unwrap());
        assert!(Interface::add_child_to(
            local_page.id(),
            &mut local_page.paragraphs,
            &mut local_para2
        )
        .unwrap());

        assert!(ForeignInterface::save(&mut foreign_page).unwrap());
        assert!(ForeignInterface::add_child_to(
            foreign_page.id(),
            &mut foreign_page.paragraphs,
            &mut foreign_para1
        )
        .unwrap());
        assert!(ForeignInterface::add_child_to(
            foreign_page.id(),
            &mut foreign_page.paragraphs,
            &mut foreign_para3
        )
        .unwrap());

        let (local_actions, foreign_actions) = Interface::compare_trees(
            &foreign_page,
            &ForeignInterface::generate_comparison_data(&foreign_page).unwrap(),
        )
        .unwrap();

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
        let (local_para1_actions, foreign_para1_actions) = Interface::compare_trees(
            &foreign_para1,
            &ForeignInterface::generate_comparison_data(&foreign_para1).unwrap(),
        )
        .unwrap();

        assert_eq!(
            local_para1_actions,
            vec![Action::Update(
                foreign_para1.id(),
                to_vec(&foreign_para1).unwrap()
            )]
        );
        assert_eq!(foreign_para1_actions, vec![]);

        // Compare para3 which doesn't exist locally
        let (local_para3_actions, foreign_para3_actions) = Interface::compare_trees(
            &foreign_para3,
            &ForeignInterface::generate_comparison_data(&foreign_para3).unwrap(),
        )
        .unwrap();

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
