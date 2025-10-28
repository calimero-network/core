use std::thread::sleep;
use std::time::Duration;

use claims::{assert_none, assert_ok};

use super::*;
use crate::entities::{Data, Element};
use crate::store::MockedStorage;
use crate::tests::common::{Page, Paragraph};

#[cfg(test)]
mod interface__public_methods {
    use super::*;

    #[test]
    fn children_of() {
        let element = Element::root();
        let mut page = Page::new_from_element("Node", element);
        assert!(MainInterface::save(&mut page).unwrap());
        assert_eq!(
            MainInterface::children_of(page.id(), &page.paragraphs).unwrap(),
            vec![]
        );

        let child1 = Element::new(&Path::new("::root::node::leaf1").unwrap(), None);
        let child2 = Element::new(&Path::new("::root::node::leaf2").unwrap(), None);
        let child3 = Element::new(&Path::new("::root::node::leaf3").unwrap(), None);
        let mut para1 = Paragraph::new_from_element("Leaf1", child1);
        let mut para2 = Paragraph::new_from_element("Leaf2", child2);
        let mut para3 = Paragraph::new_from_element("Leaf3", child3);

        assert!(!MainInterface::save(&mut page).unwrap());

        assert!(MainInterface::add_child_to(page.id(), &mut page.paragraphs, &mut para1).unwrap());
        assert!(MainInterface::add_child_to(page.id(), &mut page.paragraphs, &mut para2).unwrap());
        assert!(MainInterface::add_child_to(page.id(), &mut page.paragraphs, &mut para3).unwrap());
        assert_eq!(
            MainInterface::children_of(page.id(), &page.paragraphs).unwrap(),
            vec![para1, para2, para3]
        );
    }

    #[test]
    fn find_by_id__existent() {
        let element = Element::root();
        let mut page = Page::new_from_element("Leaf", element);
        let id = page.id();
        assert!(MainInterface::save(&mut page).unwrap());

        assert_eq!(MainInterface::find_by_id(id).unwrap(), Some(page));
    }

    #[test]
    fn find_by_id__non_existent() {
        assert_none!(MainInterface::find_by_id::<Page>(Id::random()).unwrap());
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
        let element = Element::root();
        let mut page = Page::new_from_element("Node", element);

        assert_ok!(MainInterface::save(&mut page));
    }

    #[test]
    fn save__not_dirty() {
        let element = Element::root();
        let mut page = Page::new_from_element("Node", element);

        assert!(MainInterface::save(&mut page).unwrap());
        page.element_mut().update();
        assert!(MainInterface::save(&mut page).unwrap());
    }

    #[test]
    fn save__too_old() {
        let element1 = Element::root();
        let mut page1 = Page::new_from_element("Node", element1);
        let mut page2 = page1.clone();

        assert!(MainInterface::save(&mut page1).unwrap());
        page2.element_mut().update();
        sleep(Duration::from_millis(2));
        page1.element_mut().update();
        assert!(MainInterface::save(&mut page1).unwrap());
        assert!(!MainInterface::save(&mut page2).unwrap());
    }

    #[test]
    fn save__update_existing() {
        let element = Element::root();
        let mut page = Page::new_from_element("Node", element);
        let id = page.id();
        assert!(MainInterface::save(&mut page).unwrap());

        page.storage.update();

        // TODO: Modify the element's data and check it changed

        assert!(MainInterface::save(&mut page).unwrap());
        assert_eq!(MainInterface::find_by_id(id).unwrap(), Some(page));
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
        let page = Page::new_from_element("Test Page", Element::root());
        let serialized = to_vec(&page).unwrap();
        let action = Action::Add {
            id: page.id(),
            data: serialized,
            ancestors: vec![],
            metadata: page.element().metadata,
        };

        assert!(MainInterface::apply_action(action).is_ok());

        // Verify the page was added
        let retrieved_page = MainInterface::find_by_id::<Page>(page.id()).unwrap();
        assert!(retrieved_page.is_some());
        assert_eq!(retrieved_page.unwrap().title, "Test Page");
    }

    #[test]
    fn apply_action__update() {
        let mut page = Page::new_from_element("Old Title", Element::root());
        assert!(MainInterface::save(&mut page).unwrap());

        page.title = "New Title".to_owned();
        page.element_mut().update();
        let serialized = to_vec(&page).unwrap();
        let action = Action::Update {
            id: page.id(),
            data: serialized,
            ancestors: vec![],
            metadata: page.element().metadata,
        };

        assert!(MainInterface::apply_action(action).is_ok());

        // Verify the page was updated
        let retrieved_page = MainInterface::find_by_id::<Page>(page.id())
            .unwrap()
            .unwrap();
        assert_eq!(retrieved_page.title, "New Title");
    }

    #[test]
    fn apply_action__delete() {
        let mut page = Page::new_from_element("Test Page", Element::root());
        assert!(MainInterface::save(&mut page).unwrap());

        let action = Action::DeleteRef {
            id: page.id(),
            deleted_at: time_now(),
        };

        assert!(MainInterface::apply_action(action).is_ok());

        // Verify the page was deleted
        let retrieved_page = MainInterface::find_by_id::<Page>(page.id()).unwrap();
        assert!(retrieved_page.is_none());
    }

    #[test]
    fn apply_action__delete_ref() {
        use crate::env::time_now;

        let mut page = Page::new_from_element("Test Page", Element::root());
        assert!(MainInterface::save(&mut page).unwrap());

        let action = Action::DeleteRef {
            id: page.id(),
            deleted_at: time_now(),
        };

        assert!(MainInterface::apply_action(action).is_ok());

        // Verify the page was deleted (tombstone)
        let retrieved_page = MainInterface::find_by_id::<Page>(page.id()).unwrap();
        assert!(retrieved_page.is_none());

        // Verify tombstone exists
        assert!(Index::<MainStorage>::is_deleted(page.id()).unwrap());
    }

    #[test]
    fn delete_ref_conflict_resolution() {
        use crate::env::time_now;

        let mut page = Page::new_from_element("Test Page", Element::root());
        assert!(MainInterface::save(&mut page).unwrap());

        // Update page (newer timestamp)
        page.title = "Updated Page".to_owned();
        page.element_mut().update();
        assert!(MainInterface::save(&mut page).unwrap());

        let update_time = *page.element().metadata.updated_at;

        // Try to delete with older timestamp
        let old_delete = Action::DeleteRef {
            id: page.id(),
            deleted_at: update_time - 1000, // Older than update
        };

        assert!(MainInterface::apply_action(old_delete).is_ok());

        // Page should still exist (update wins)
        let retrieved = MainInterface::find_by_id::<Page>(page.id()).unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().title, "Updated Page");

        // Now delete with newer timestamp
        let new_delete = Action::DeleteRef {
            id: page.id(),
            deleted_at: update_time + 1000, // Newer than update
        };

        assert!(MainInterface::apply_action(new_delete).is_ok());

        // Page should be deleted (deletion wins)
        let retrieved = MainInterface::find_by_id::<Page>(page.id()).unwrap();
        assert!(retrieved.is_none());
    }

    #[test]
    fn apply_action__compare() {
        let page = Page::new_from_element("Test Page", Element::root());
        let action = Action::Compare { id: page.id() };

        // Compare should fail
        assert!(MainInterface::apply_action(action).is_err());
    }

    #[test]
    fn apply_action__non_existent_update() {
        let page = Page::new_from_element("Test Page", Element::root());
        let serialized = to_vec(&page).unwrap();
        let action = Action::Update {
            id: page.id(),
            data: serialized,
            ancestors: vec![],
            metadata: page.element().metadata,
        };

        // Updating a non-existent page should still succeed (it will be added)
        assert!(MainInterface::apply_action(action).is_ok());

        // Verify the page was added
        let retrieved_page = MainInterface::find_by_id::<Page>(page.id()).unwrap();
        assert!(retrieved_page.is_some());
        assert_eq!(retrieved_page.unwrap().title, "Test Page");
    }
}

#[cfg(test)]
mod interface__comparison {
    use super::*;

    type ForeignInterface = Interface<MockedStorage<0>>;

    fn compare_trees<D: Data>(
        foreign: Option<&D>,
        comparison_data: ComparisonData,
    ) -> Result<(Vec<Action>, Vec<Action>), StorageError> {
        MainInterface::compare_trees(
            foreign
                .map(to_vec)
                .transpose()
                .map_err(StorageError::SerializationError)?,
            comparison_data,
        )
    }

    #[test]
    fn compare_trees__identical() {
        let element = Element::root();
        let mut local = Page::new_from_element("Test Page", element);
        let mut foreign = local.clone();

        assert!(MainInterface::save(&mut local).unwrap());
        assert!(ForeignInterface::save(&mut foreign).unwrap());
        assert_eq!(
            local.element().merkle_hash(),
            foreign.element().merkle_hash()
        );

        let result = compare_trees(
            Some(&foreign),
            ForeignInterface::generate_comparison_data(Some(foreign.id())).unwrap(),
        )
        .unwrap();
        assert_eq!(result, (vec![], vec![]));
    }

    #[test]
    fn compare_trees__local_newer() {
        let element = Element::root();
        let mut local = Page::new_from_element("Test Page", element.clone());
        let mut foreign = Page::new_from_element("Old Test Page", element);

        assert!(ForeignInterface::save(&mut foreign).unwrap());

        // Make local newer
        sleep(Duration::from_millis(10));
        local.element_mut().update();
        assert!(MainInterface::save(&mut local).unwrap());

        let result = compare_trees(
            Some(&foreign),
            ForeignInterface::generate_comparison_data(Some(foreign.id())).unwrap(),
        )
        .unwrap();
        assert_eq!(
            result,
            (
                vec![],
                vec![Action::Update {
                    id: local.id(),
                    data: to_vec(&local).unwrap(),
                    ancestors: vec![],
                    metadata: local.element().metadata,
                }]
            )
        );
    }

    #[test]
    fn compare_trees__foreign_newer() {
        let element = Element::root();
        let mut local = Page::new_from_element("Old Test Page", element.clone());
        let mut foreign = Page::new_from_element("Test Page", element);

        assert!(MainInterface::save(&mut local).unwrap());

        // Make foreign newer
        sleep(Duration::from_millis(10));
        foreign.element_mut().update();
        assert!(ForeignInterface::save(&mut foreign).unwrap());

        let result = compare_trees(
            Some(&foreign),
            ForeignInterface::generate_comparison_data(Some(foreign.id())).unwrap(),
        )
        .unwrap();
        assert_eq!(
            result,
            (
                vec![Action::Update {
                    id: foreign.id(),
                    data: to_vec(&foreign).unwrap(),
                    ancestors: vec![],
                    metadata: foreign.element().metadata,
                }],
                vec![]
            )
        );
    }

    #[test]
    fn compare_trees__with_collections() {
        let page_element = Element::root();
        let para1_element = Element::new(&Path::new("::root::node::leaf1").unwrap(), None);
        let para2_element = Element::new(&Path::new("::root::node::leaf2").unwrap(), None);
        let para3_element = Element::new(&Path::new("::root::node::leaf3").unwrap(), None);

        let mut local_page = Page::new_from_element("Local Page", page_element.clone());
        let mut local_para1 =
            Paragraph::new_from_element("Local Paragraph 1", para1_element.clone());
        let mut local_para2 = Paragraph::new_from_element("Local Paragraph 2", para2_element);

        let mut foreign_page = Page::new_from_element("Foreign Page", page_element);
        let mut foreign_para1 = Paragraph::new_from_element("Updated Paragraph 1", para1_element);
        let mut foreign_para3 = Paragraph::new_from_element("Foreign Paragraph 3", para3_element);

        assert!(MainInterface::save(&mut local_page).unwrap());
        assert!(MainInterface::add_child_to(
            local_page.id(),
            &mut local_page.paragraphs,
            &mut local_para1
        )
        .unwrap());
        assert!(MainInterface::add_child_to(
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

        let (local_actions, foreign_actions) = compare_trees(
            Some(&foreign_page),
            ForeignInterface::generate_comparison_data(Some(foreign_page.id())).unwrap(),
        )
        .unwrap();

        assert_eq!(
            local_actions,
            vec![
                // Page needs update due to different child structure
                Action::Update {
                    id: foreign_page.id(),
                    data: to_vec(&foreign_page).unwrap(),
                    ancestors: vec![],
                    metadata: foreign_page.element().metadata,
                },
                // Para1 needs comparison due to different hash
                Action::Compare {
                    id: local_para1.id()
                },
            ]
        );
        local_para2.element_mut().is_dirty = true;
        assert_eq!(
            foreign_actions,
            vec![
                // Para1 needs comparison due to different hash
                Action::Compare {
                    id: local_para1.id()
                },
                // Para2 needs to be added to foreign
                Action::Add {
                    id: local_para2.id(),
                    data: to_vec(&local_para2).unwrap(),
                    ancestors: vec![],
                    metadata: local_para2.element().metadata,
                },
                // Para3 needs to be added locally, but we don't have the data, so we compare
                Action::Compare {
                    id: foreign_para3.id()
                },
            ]
        );

        // Compare the updated para1
        let (local_para1_actions, foreign_para1_actions) = compare_trees(
            Some(&foreign_para1),
            ForeignInterface::generate_comparison_data(Some(foreign_para1.id())).unwrap(),
        )
        .unwrap();

        // Here, para1 has been updated, but also para2 is present locally and para3
        // is present remotely. So the ancestor hashes will not match, and will
        // trigger a recomparison.
        let local_para1_ancestor_hash = {
            let Action::Update { ancestors, .. } = local_para1_actions[0].clone() else {
                panic!("Expected an update action");
            };
            ancestors[0].merkle_hash()
        };
        assert_ne!(
            local_para1_ancestor_hash,
            foreign_page.element().merkle_hash()
        );
        assert_eq!(
            local_para1_actions,
            vec![Action::Update {
                id: foreign_para1.id(),
                data: to_vec(&foreign_para1).unwrap(),
                ancestors: vec![ChildInfo::new(
                    foreign_page.id(),
                    local_para1_ancestor_hash,
                    local_page.element().metadata
                )],
                metadata: foreign_para1.element().metadata,
            }]
        );
        assert_eq!(foreign_para1_actions, vec![]);

        // Compare para3 which doesn't exist locally
        let (local_para3_actions, foreign_para3_actions) = compare_trees(
            Some(&foreign_para3),
            ForeignInterface::generate_comparison_data(Some(foreign_para3.id())).unwrap(),
        )
        .unwrap();

        // Here, para3 is present remotely but not locally, and also para2 is
        // present locally and not remotely, and para1 has been updated. So the
        // ancestor hashes will not match, and will trigger a recomparison.
        let local_para3_ancestor_hash = {
            let Action::Add { ancestors, .. } = local_para3_actions[0].clone() else {
                panic!("Expected an update action");
            };
            ancestors[0].merkle_hash()
        };
        assert_ne!(
            local_para3_ancestor_hash,
            foreign_page.element().merkle_hash()
        );
        assert_eq!(
            local_para3_actions,
            vec![Action::Add {
                id: foreign_para3.id(),
                data: to_vec(&foreign_para3).unwrap(),
                ancestors: vec![ChildInfo::new(
                    foreign_page.id(),
                    local_para3_ancestor_hash,
                    foreign_page.element().metadata
                )],
                metadata: foreign_para3.element().metadata,
            }]
        );
        assert_eq!(foreign_para3_actions, vec![]);
    }
}

#[cfg(test)]
mod snapshot_and_resync {
    use super::*;
    use crate::address::Path;
    use crate::snapshot::{apply_snapshot, generate_full_snapshot, generate_snapshot};
    use crate::tests::common::{Page, Paragraph};

    // Use MockedStorage for snapshot tests (has working storage_iter_keys)
    type TestStorage = MockedStorage<1000>;
    type TestInterface = Interface<TestStorage>;

    #[test]
    fn test_generate_snapshot() {
        // Create root page
        let mut page = Page::new_from_element("Test Page", Element::root());

        // Create paragraphs with unique paths
        let para1_path = Path::new("::para1").unwrap();
        let para2_path = Path::new("::para2").unwrap();
        let mut para1 = Paragraph::new_from_element("Para 1", Element::new(&para1_path, None));
        let mut para2 = Paragraph::new_from_element("Para 2", Element::new(&para2_path, None));

        TestInterface::save(&mut page).unwrap();
        TestInterface::add_child_to(page.id(), &page.paragraphs, &mut para1).unwrap();
        TestInterface::add_child_to(page.id(), &page.paragraphs, &mut para2).unwrap();

        // Generate snapshot
        let snapshot = generate_snapshot::<TestStorage>().unwrap();

        // Verify snapshot contains data
        assert!(snapshot.entity_count > 0, "Should have entities");
        assert!(snapshot.index_count > 0, "Should have indexes");
        assert!(snapshot.timestamp > 0, "Should have timestamp");

        // Verify specific entities are included
        let entry_ids: Vec<_> = snapshot.entries.iter().map(|(id, _)| *id).collect();
        assert!(entry_ids.contains(&page.id()), "Should include page");
        assert!(entry_ids.contains(&para1.id()), "Should include para1");
        assert!(entry_ids.contains(&para2.id()), "Should include para2");
    }

    #[test]
    fn test_apply_snapshot() {
        type ForeignStorage = MockedStorage<99>;
        type ForeignInterface = Interface<ForeignStorage>;

        // Create data on foreign node - page as root, para as child
        let mut foreign_page = Page::new_from_element("Foreign Page", Element::root());
        let para_path = Path::new("::foreign_para").unwrap();
        let mut foreign_para =
            Paragraph::new_from_element("Foreign Para", Element::new(&para_path, None));

        ForeignInterface::save(&mut foreign_page).unwrap();
        ForeignInterface::add_child_to(
            foreign_page.id(),
            &foreign_page.paragraphs,
            &mut foreign_para,
        )
        .unwrap();

        // Generate snapshot from foreign
        let snapshot = generate_snapshot::<ForeignStorage>().unwrap();

        // Apply snapshot to TestStorage (which is empty)
        assert!(apply_snapshot::<TestStorage>(&snapshot).is_ok());

        // Verify data was restored
        let retrieved_page = TestInterface::find_by_id::<Page>(foreign_page.id()).unwrap();
        assert!(retrieved_page.is_some(), "Page should be restored");
        assert_eq!(retrieved_page.unwrap().title, "Foreign Page");

        let retrieved_para = TestInterface::find_by_id::<Paragraph>(foreign_para.id()).unwrap();
        assert!(retrieved_para.is_some(), "Paragraph should be restored");
        assert_eq!(retrieved_para.unwrap().text, "Foreign Para");
    }

    #[test]
    fn test_snapshot_excludes_tombstones() {
        use crate::index::Index;

        // Create parent page as root
        let mut page = Page::new_from_element("Parent Page", Element::root());

        // Create paragraphs with unique paths
        let para1_path = Path::new("::para1").unwrap();
        let para2_path = Path::new("::para2").unwrap();
        let mut para1 = Paragraph::new_from_element("Para 1", Element::new(&para1_path, None));
        let mut para2 = Paragraph::new_from_element("Para 2", Element::new(&para2_path, None));

        TestInterface::save(&mut page).unwrap();
        TestInterface::add_child_to(page.id(), &page.paragraphs, &mut para1).unwrap();
        TestInterface::add_child_to(page.id(), &page.paragraphs, &mut para2).unwrap();

        // Verify different IDs
        assert_ne!(para1.id(), para2.id());
        assert_ne!(para1.id(), page.id());

        // Delete para1 (creates tombstone)
        Index::<TestStorage>::mark_deleted(para1.id(), crate::env::time_now()).unwrap();

        // Generate regular snapshot (excludes tombstones)
        let snapshot = generate_snapshot::<TestStorage>().unwrap();

        // Verify para1 (tombstone) is NOT in snapshot
        let index_ids: Vec<_> = snapshot.indexes.iter().map(|(id, _)| *id).collect();
        assert!(
            !index_ids.contains(&para1.id()),
            "Tombstone should be excluded from network snapshot"
        );
        assert!(
            index_ids.contains(&para2.id()),
            "Active entity should be included"
        );
        assert!(index_ids.contains(&page.id()), "Parent should be included");

        // Generate full snapshot (includes tombstones)
        let full_snapshot = generate_full_snapshot::<TestStorage>().unwrap();
        let full_index_ids: Vec<_> = full_snapshot.indexes.iter().map(|(id, _)| *id).collect();
        assert!(
            full_index_ids.contains(&para1.id()),
            "Tombstone should be included in full snapshot"
        );
    }

    #[test]
    fn test_snapshot_round_trip() {
        type SourceStorage = MockedStorage<100>;
        type SourceInterface = Interface<SourceStorage>;
        type TargetStorage = MockedStorage<101>;
        type TargetInterface = Interface<TargetStorage>;

        // Create data on source
        let mut page = Page::new_from_element("Source Page", Element::root());
        let para_path = Path::new("::source_para").unwrap();
        let mut para = Paragraph::new_from_element("Source Para", Element::new(&para_path, None));

        SourceInterface::save(&mut page).unwrap();
        SourceInterface::add_child_to(page.id(), &page.paragraphs, &mut para).unwrap();

        // Generate snapshot
        let snapshot = generate_snapshot::<SourceStorage>().unwrap();

        // Apply to target
        apply_snapshot::<TargetStorage>(&snapshot).unwrap();

        // Verify data on target matches source
        let target_page = TargetInterface::find_by_id::<Page>(page.id()).unwrap();
        assert!(target_page.is_some());
        assert_eq!(target_page.unwrap().title, "Source Page");

        let target_para = TargetInterface::find_by_id::<Paragraph>(para.id()).unwrap();
        assert!(target_para.is_some());
        assert_eq!(target_para.unwrap().text, "Source Para");
    }
}
