#![allow(unused_results)] // Test code doesn't need to check all return values

use std::thread::sleep;
use std::time::Duration;

use claims::{assert_none, assert_ok};
use sha2::{Digest, Sha256};

use super::*;
use crate::constants::DRIFT_TOLERANCE_NANOS;
use crate::entities::{Data, Element, SignatureData, StorageType};
use crate::store::MockedStorage;
use crate::tests::common::{Page, Paragraph};

#[cfg(test)]
mod interface__public_methods {
    use super::*;
    use serial_test::serial;

    #[test]
    fn children_of() {
        let element = Element::root();
        let mut page = Page::new_from_element("Node", element);
        assert!(MainInterface::save(&mut page).unwrap());
        assert_eq!(
            MainInterface::children_of::<Paragraph>(page.id()).unwrap(),
            vec![]
        );

        let child1 = Element::new(None);
        let child2 = Element::new(None);
        let child3 = Element::new(None);
        let mut para1 = Paragraph::new_from_element("Leaf1", child1);
        let mut para2 = Paragraph::new_from_element("Leaf2", child2);
        let mut para3 = Paragraph::new_from_element("Leaf3", child3);

        assert!(!MainInterface::save(&mut page).unwrap());

        assert!(MainInterface::add_child_to(page.id(), &mut para1).unwrap());
        assert!(MainInterface::add_child_to(page.id(), &mut para2).unwrap());
        assert!(MainInterface::add_child_to(page.id(), &mut para3).unwrap());

        let mut children: Vec<Paragraph> = MainInterface::children_of(page.id()).unwrap();
        let mut expected = vec![para1, para2, para3];

        // Sort both by ID for deterministic comparison
        children.sort_by_key(|p| p.id());
        expected.sort_by_key(|p| p.id());

        assert_eq!(children, expected);
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
    fn save__basic() {
        let element = Element::root();
        let mut page = Page::new_from_element("Node", element);

        assert_ok!(MainInterface::save(&mut page));
    }

    #[test]
    fn save__not_dirty() {
        crate::tests::common::register_test_merge_functions();
        let element = Element::root();
        let mut page = Page::new_from_element("Node", element);

        assert!(MainInterface::save(&mut page).unwrap());
        page.element_mut().update();
        assert!(MainInterface::save(&mut page).unwrap());
    }

    #[test]
    #[serial]
    fn save__too_old() {
        crate::tests::common::register_test_merge_functions();
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
        crate::tests::common::register_test_merge_functions();
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
            metadata: page.element().metadata.clone(),
        };

        assert!(MainInterface::apply_action(action, &ApplyContext::empty()).is_ok());

        // Verify the page was added
        let retrieved_page = MainInterface::find_by_id::<Page>(page.id()).unwrap();
        assert!(retrieved_page.is_some());
        assert_eq!(retrieved_page.unwrap().title, "Test Page");
    }

    #[test]
    fn apply_action__update() {
        crate::tests::common::register_test_merge_functions();
        let mut page = Page::new_from_element("Old Title", Element::root());
        assert!(MainInterface::save(&mut page).unwrap());

        page.title = "New Title".to_owned();
        page.element_mut().update();
        let serialized = to_vec(&page).unwrap();
        let action = Action::Update {
            id: page.id(),
            data: serialized,
            ancestors: vec![],
            metadata: page.element().metadata.clone(),
        };

        assert!(MainInterface::apply_action(action, &ApplyContext::empty()).is_ok());

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
            metadata: Metadata::default(),
        };

        assert!(MainInterface::apply_action(action, &ApplyContext::empty()).is_ok());

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
            metadata: Metadata::default(),
        };

        assert!(MainInterface::apply_action(action, &ApplyContext::empty()).is_ok());

        // Verify the page was deleted (tombstone)
        let retrieved_page = MainInterface::find_by_id::<Page>(page.id()).unwrap();
        assert!(retrieved_page.is_none());

        // Verify tombstone exists
        assert!(Index::<MainStorage>::is_deleted(page.id()).unwrap());
    }

    #[test]
    fn delete_ref_conflict_resolution() {
        crate::tests::common::register_test_merge_functions();
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
            metadata: Metadata::default(),
        };

        assert!(MainInterface::apply_action(old_delete, &ApplyContext::empty()).is_ok());

        // Page should still exist (update wins)
        let retrieved = MainInterface::find_by_id::<Page>(page.id()).unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().title, "Updated Page");

        // Now delete with newer timestamp
        let new_delete = Action::DeleteRef {
            id: page.id(),
            deleted_at: update_time + 1000, // Newer than update
            metadata: Metadata::default(),
        };

        assert!(MainInterface::apply_action(new_delete, &ApplyContext::empty()).is_ok());

        // Page should be deleted (deletion wins)
        let retrieved = MainInterface::find_by_id::<Page>(page.id()).unwrap();
        assert!(retrieved.is_none());
    }

    #[test]
    fn apply_action__compare() {
        let page = Page::new_from_element("Test Page", Element::root());
        let action = Action::Compare { id: page.id() };

        // Compare should fail
        assert!(MainInterface::apply_action(action, &ApplyContext::empty()).is_err());
    }

    #[test]
    fn apply_action__non_existent_update() {
        let page = Page::new_from_element("Test Page", Element::root());
        let serialized = to_vec(&page).unwrap();
        let action = Action::Update {
            id: page.id(),
            data: serialized,
            ancestors: vec![],
            metadata: page.element().metadata.clone(),
        };

        // Updating a non-existent page should still succeed (it will be added)
        assert!(MainInterface::apply_action(action, &ApplyContext::empty()).is_ok());

        // Verify the page was added
        let retrieved_page = MainInterface::find_by_id::<Page>(page.id()).unwrap();
        assert!(retrieved_page.is_some());
        assert_eq!(retrieved_page.unwrap().title, "Test Page");
    }

    // Regression for #2356 item 1: a stale-by-HLC apply (incoming.updated_at <
    // stored.updated_at) hits the `save_internal -> None` short-circuit. The
    // apply must still enqueue Action::Compare so the receiver's current
    // Merkle state for this entity propagates to peers — otherwise two nodes
    // that each hold the locally-newer side of a concurrent CRDT merge keep
    // dropping each other's deltas, and root-hash convergence stalls until an
    // unrelated trigger forces a hash-comparison sweep.
    #[test]
    fn apply_action__stale_update_still_emits_compare() {
        use crate::delta::{commit_causal_delta, reset_delta_context};

        crate::tests::common::register_test_merge_functions();
        reset_delta_context();

        // Seed an entity locally at time T2.
        let mut page = Page::new_from_element("Test Page", Element::root());
        let id = page.id();
        assert!(MainInterface::save(&mut page).unwrap());
        let stored_updated_at = *page.element().metadata.updated_at;
        // Discard actions emitted by the local save — the assertion below is
        // only about what the stale apply contributes.
        reset_delta_context();

        // Build a remote-style Update with metadata at T1 < T2.
        let mut stale_metadata = page.element().metadata.clone();
        stale_metadata.set_updated_at(stored_updated_at.saturating_sub(1_000_000));
        let stale_action = Action::Update {
            id,
            data: to_vec(&page).unwrap(),
            ancestors: vec![],
            metadata: stale_metadata,
        };

        assert!(MainInterface::apply_action(stale_action, &ApplyContext::empty()).is_ok());

        let delta = commit_causal_delta(&[0; 32])
            .unwrap()
            .expect("stale apply must still emit a delta (Action::Compare)");
        assert!(
            delta
                .actions
                .iter()
                .any(|a| matches!(a, Action::Compare { id: cid } if *cid == id)),
            "expected Action::Compare for id={id} after stale apply, got: {:?}",
            delta.actions
        );
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
                    metadata: local.element().metadata.clone(),
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
                    metadata: foreign.element().metadata.clone(),
                }],
                vec![]
            )
        );
    }

    #[test]
    fn compare_trees__with_collections() {
        let page_element = Element::root();
        let para1_element = Element::new(None);
        let para2_element = Element::new(None);
        let para3_element = Element::new(None);

        let mut local_page = Page::new_from_element("Local Page", page_element.clone());
        let mut local_para1 =
            Paragraph::new_from_element("Local Paragraph 1", para1_element.clone());
        let mut local_para2 = Paragraph::new_from_element("Local Paragraph 2", para2_element);

        let mut foreign_page = Page::new_from_element("Foreign Page", page_element);
        let mut foreign_para1 = Paragraph::new_from_element("Updated Paragraph 1", para1_element);
        let mut foreign_para3 = Paragraph::new_from_element("Foreign Paragraph 3", para3_element);

        assert!(MainInterface::save(&mut local_page).unwrap());
        assert!(MainInterface::add_child_to(local_page.id(), &mut local_para1).unwrap());
        assert!(MainInterface::add_child_to(local_page.id(), &mut local_para2).unwrap());

        assert!(ForeignInterface::save(&mut foreign_page).unwrap());
        assert!(ForeignInterface::add_child_to(foreign_page.id(), &mut foreign_para1).unwrap());
        assert!(ForeignInterface::add_child_to(foreign_page.id(), &mut foreign_para3).unwrap());

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
                    metadata: foreign_page.element().metadata.clone(),
                },
                // Para1 needs comparison due to different hash
                Action::Compare {
                    id: local_para1.id()
                },
            ]
        );
        local_para2.element_mut().is_dirty = true;
        // Para2 is a local-only child within a collection both sides share, so
        // it takes the "child missing from foreign" arm. Its Add must carry
        // para2's *own* ancestor chain — i.e. its immediate parent, the page —
        // exactly like the "collection entirely missing" arm does for para3
        // below. Regression guard: this previously asserted `ancestors: vec![]`,
        // which only held because the parent is the root page (whose ancestors
        // are empty); the action was being built from the parent's ancestors
        // instead of the child's, orphaning para2 on the receiver.
        // Expected ancestor hash, derived independently of the action under
        // test: the page's own full Merkle hash, which is what
        // `get_ancestors_of` records for the parent entry.
        let expected_page_hash = MainInterface::generate_comparison_data(Some(local_page.id()))
            .unwrap()
            .full_hash;
        let local_para2_ancestor_hash = {
            let Action::Add { ancestors, .. } = foreign_actions[1].clone() else {
                panic!("Expected para2 to be added to foreign");
            };
            assert_eq!(ancestors.len(), 1, "para2 must carry exactly its parent");
            assert_eq!(
                ancestors[0].id(),
                local_page.id(),
                "para2's ancestor must be its immediate parent (the page)"
            );
            assert_eq!(
                ancestors[0].merkle_hash(),
                expected_page_hash,
                "para2's ancestor hash must be the page's full Merkle hash"
            );
            ancestors[0].merkle_hash()
        };
        assert_eq!(
            foreign_actions,
            vec![
                // Para1 needs comparison due to different hash
                Action::Compare {
                    id: local_para1.id()
                },
                // Para2 needs to be added to foreign, under its parent page
                Action::Add {
                    id: local_para2.id(),
                    data: to_vec(&local_para2).unwrap(),
                    ancestors: vec![ChildInfo::new(
                        local_page.id(),
                        local_para2_ancestor_hash,
                        local_page.element().metadata.clone(),
                    )],
                    metadata: local_para2.element().metadata.clone(),
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
                    local_page.element().metadata.clone(),
                )],
                metadata: foreign_para1.element().metadata.clone(),
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
                    foreign_page.element().metadata.clone(),
                )],
                metadata: foreign_para3.element().metadata.clone(),
            }]
        );
        assert_eq!(foreign_para3_actions, vec![]);
    }
}

#[cfg(test)]
mod snapshot_and_resync {
    use super::*;
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
        let mut para1 = Paragraph::new_from_element("Para 1", Element::new(None));
        let mut para2 = Paragraph::new_from_element("Para 2", Element::new(None));

        TestInterface::save(&mut page).unwrap();
        TestInterface::add_child_to(page.id(), &mut para1).unwrap();
        TestInterface::add_child_to(page.id(), &mut para2).unwrap();

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
        let mut foreign_para = Paragraph::new_from_element("Foreign Para", Element::new(None));

        ForeignInterface::save(&mut foreign_page).unwrap();
        ForeignInterface::add_child_to(foreign_page.id(), &mut foreign_para).unwrap();

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
        let mut para1 = Paragraph::new_from_element("Para 1", Element::new(None));
        let mut para2 = Paragraph::new_from_element("Para 2", Element::new(None));

        TestInterface::save(&mut page).unwrap();
        TestInterface::add_child_to(page.id(), &mut para1).unwrap();
        TestInterface::add_child_to(page.id(), &mut para2).unwrap();

        // Verify different IDs
        assert_ne!(para1.id(), para2.id());
        assert_ne!(para1.id(), page.id());

        // Delete para1 (creates tombstone)
        Index::<TestStorage>::mark_deleted(para1.id(), time_now()).unwrap();

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
        let mut para = Paragraph::new_from_element("Source Para", Element::new(None));

        SourceInterface::save(&mut page).unwrap();
        SourceInterface::add_child_to(page.id(), &mut para).unwrap();

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

/// Tests for User storage signature verification.
///
/// User storage requires cryptographic signatures for all remote actions.
/// These tests verify signature verification, replay protection, and owner checks.
#[cfg(test)]
mod user_storage_signature_verification {
    use super::*;
    use crate::env;
    use crate::tests::common::{
        create_signed_user_add_action, create_signed_user_update_action, create_test_keypair,
    };

    #[test]
    fn user_action_with_valid_signature_succeeds() {
        env::reset_for_testing();

        let (signing_key, owner) = create_test_keypair();

        // Create user-owned page
        let mut element = Element::root();
        element.set_user_domain(owner);
        let page = Page::new_from_element("User Page", element);
        let serialized = to_vec(&page).unwrap();

        let nonce = env::time_now();
        let action =
            create_signed_user_add_action(&signing_key, owner, page.id(), serialized, nonce);

        // Valid signature should succeed
        assert!(MainInterface::apply_action(action, &ApplyContext::empty()).is_ok());

        // Verify the page was added
        let retrieved = MainInterface::find_by_id::<Page>(page.id()).unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().title, "User Page");
    }

    #[test]
    fn user_action_with_invalid_signature_fails() {
        env::reset_for_testing();

        let (_, owner) = create_test_keypair();
        let (wrong_signing_key, _) = create_test_keypair(); // Different key

        let mut element = Element::root();
        element.set_user_domain(owner);
        let page = Page::new_from_element("User Page", element);
        let serialized = to_vec(&page).unwrap();

        let nonce = env::time_now();

        // Sign with WRONG key
        let action =
            create_signed_user_add_action(&wrong_signing_key, owner, page.id(), serialized, nonce);

        // Invalid signature should fail
        let result = MainInterface::apply_action(action, &ApplyContext::empty());
        assert!(result.is_err());
        match result {
            Err(StorageError::InvalidSignature) => {}
            other => panic!("Expected InvalidSignature error, got {:?}", other),
        }
    }

    #[test]
    fn user_action_without_signature_fails() {
        env::reset_for_testing();

        let (_, owner) = create_test_keypair();

        let mut element = Element::root();
        element.set_user_domain(owner);
        let page = Page::new_from_element("User Page", element);
        let serialized = to_vec(&page).unwrap();

        let timestamp = env::time_now();

        // Create action WITHOUT signature
        let action = Action::Add {
            id: page.id(),
            data: serialized,
            ancestors: vec![],
            metadata: Metadata {
                created_at: timestamp,
                updated_at: timestamp.into(),
                storage_type: StorageType::User {
                    owner,
                    signature_data: None, // No signature!
                },
                crdt_type: None,
                field_name: None,
                schema_version: None,
            },
        };

        // Missing signature should fail
        let result = MainInterface::apply_action(action, &ApplyContext::empty());
        assert!(result.is_err());
        match result {
            Err(StorageError::InvalidData(msg)) => {
                assert!(
                    msg.contains("signed"),
                    "Error should mention signing: {}",
                    msg
                );
            }
            other => panic!("Expected InvalidData error, got {:?}", other),
        }
    }

    #[test]
    fn user_action_with_corrupted_signature_fails() {
        env::reset_for_testing();

        let (signing_key, owner) = create_test_keypair();

        let mut element = Element::root();
        element.set_user_domain(owner);
        let page = Page::new_from_element("User Page", element);
        let serialized = to_vec(&page).unwrap();

        let nonce = env::time_now();
        let mut action =
            create_signed_user_add_action(&signing_key, owner, page.id(), serialized, nonce);

        // Corrupt the signature
        if let Action::Add {
            ref mut metadata, ..
        } = action
        {
            if let StorageType::User {
                ref mut signature_data,
                ..
            } = metadata.storage_type
            {
                if let Some(ref mut sig_data) = signature_data {
                    sig_data.signature[0] ^= 0xFF; // Flip bits
                    sig_data.signature[31] ^= 0xFF;
                }
            }
        }

        // Corrupted signature should fail
        let result = MainInterface::apply_action(action, &ApplyContext::empty());
        assert!(result.is_err());
        match result {
            Err(StorageError::InvalidSignature) => {}
            other => panic!("Expected InvalidSignature error, got {:?}", other),
        }
    }

    #[test]
    fn user_update_with_valid_signature_succeeds() {
        crate::tests::common::register_test_merge_functions();
        env::reset_for_testing();

        let (signing_key, owner) = create_test_keypair();

        // First, create the entity
        let mut element = Element::root();
        element.set_user_domain(owner);
        let page = Page::new_from_element("Original Title", element);
        let serialized = to_vec(&page).unwrap();

        let nonce1 = env::time_now();
        let action1 =
            create_signed_user_add_action(&signing_key, owner, page.id(), serialized, nonce1);
        assert!(MainInterface::apply_action(action1, &ApplyContext::empty()).is_ok());

        // Wait a bit to ensure different timestamp
        sleep(Duration::from_millis(2));

        // Now update it
        let mut updated_page = page.clone();
        updated_page.title = "Updated Title".to_owned();
        let updated_serialized = to_vec(&updated_page).unwrap();

        let nonce2 = env::time_now();
        let action2 = create_signed_user_update_action(
            &signing_key,
            owner,
            page.id(),
            updated_serialized,
            nonce2,
            page.element().created_at(),
        );

        assert!(MainInterface::apply_action(action2, &ApplyContext::empty()).is_ok());

        // Verify the update
        let retrieved = MainInterface::find_by_id::<Page>(page.id()).unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().title, "Updated Title");
    }
}

/// Tests for User storage replay protection (nonce checks).
///
/// Replay protection ensures that old actions cannot be re-applied,
/// preventing replay attacks where an attacker resends old signed messages.
#[cfg(test)]
mod user_storage_replay_protection {
    use ed25519_dalek::Signer;

    use super::*;
    use crate::env;
    use crate::tests::common::{
        create_signed_user_add_action, create_signed_user_update_action, create_test_keypair,
    };

    #[test]
    fn replay_same_signed_action_is_idempotent() {
        // Semantic relaxed in the commit addressing
        // HashComparison's recurse-into-common-children re-delivery:
        // same nonce + valid signature = byte-identical action (the
        // signature commits to `(id, data, nonce, storage_type)`,
        // so equal nonce + valid signature ⇒ equal payload).
        // Re-applying the SAME signed action is idempotent (a
        // bytewise no-op at `save_internal`), not a replay attack.
        // The pre-fix strict `<=` rejected this and blocked
        // post-divergence sync convergence.
        //
        // Strict rejection semantics remain for STRICTLY-LOWER
        // nonces (see `replay_with_lower_nonce_fails` below) and
        // for `DeleteRef` (where same-nonce delete is destructive
        // and shouldn't occur in legitimate flows).
        //
        // **Why this test constructs the action manually** instead
        // of using `create_signed_user_add_action`: that helper
        // sets `metadata.updated_at = time_now()` but takes
        // `sig_data.nonce` as a separate parameter — so the two
        // can diverge in tests. In production, `save_raw` sets
        // both from the same HLC value, so they're always equal.
        // The idempotent re-apply path keys off
        // `sig_data.nonce == stored.metadata.updated_at`, so the
        // production invariant has to hold for the test to
        // exercise the right code path.
        env::reset_for_testing();

        let (signing_key, owner) = create_test_keypair();

        let mut element = Element::root();
        element.set_user_domain(owner);
        let page = Page::new_from_element("Page", element);
        let serialized = to_vec(&page).unwrap();

        // Manually construct the action with `sig_data.nonce ==
        // metadata.updated_at`, mirroring `save_raw`'s production
        // invariant.
        let hlc = env::time_now();
        let mut metadata = Metadata {
            created_at: hlc,
            updated_at: hlc.into(),
            storage_type: StorageType::User {
                owner,
                signature_data: Some(SignatureData {
                    signature: [0u8; 64], // placeholder, set below
                    nonce: hlc,
                    signer: None,
                }),
            },
            crdt_type: None,
            field_name: None,
            schema_version: None,
        };
        let mut action = Action::Add {
            id: page.id(),
            data: serialized,
            ancestors: vec![],
            metadata: metadata.clone(),
        };
        let payload = action.payload_for_signing();
        let signature = signing_key.sign(&payload).to_bytes();
        if let StorageType::User {
            signature_data: Some(ref mut sd),
            ..
        } = metadata.storage_type
        {
            sd.signature = signature;
        }
        if let Action::Add {
            metadata: ref mut m,
            ..
        } = action
        {
            *m = metadata;
        }

        // First apply succeeds.
        assert!(MainInterface::apply_action(action.clone(), &ApplyContext::empty()).is_ok());

        sleep(Duration::from_millis(2));

        // Re-applying the exact same signed action must be
        // idempotent — not a NonceReplay rejection.
        let result = MainInterface::apply_action(action, &ApplyContext::empty());
        assert!(
            result.is_ok(),
            "re-applying same signed action must be idempotent, got {result:?}"
        );
    }

    #[test]
    fn replay_with_lower_nonce_is_silent_noop_for_upsert() {
        // Upsert (Add/Update) used to reject lower-nonce actions as
        // `NonceReplay`. That rejection bubbles through
        // `Root::sync().expect("fatal: sync failed")` in the SDK
        // macro and aborts the entire WASM sync batch, which blocks
        // post-divergence convergence: a HashComparison or DAG
        // catchup that re-delivers a now-stale-but-authentic leaf
        // (the newer twin already arrived via gossipsub) would kill
        // the whole sync.
        //
        // The new contract: verify the signature first (an
        // unauthenticated stale action still rejects as
        // `InvalidSignature`), then on `new_nonce <= last_nonce`
        // silently skip with Ok(()). The state isn't downgraded —
        // we just no-op on the stale action, leaving the newer
        // local state intact. The owner-signature-replay test
        // below (`replay_signature_with_different_data_rejected`)
        // covers the security property: forged data can't slip
        // through because the signature commits to `(id, data,
        // nonce)` and verify still runs.
        //
        // The DeleteRef path keeps the strict `Err(NonceReplay)` on
        // `<=` — see `tombstone_replay_with_lower_nonce_fails`.
        env::reset_for_testing();

        let (signing_key, owner) = create_test_keypair();

        let mut element = Element::root();
        element.set_user_domain(owner);
        let page = Page::new_from_element("Page", element);
        let serialized = to_vec(&page).unwrap();

        let nonce1 = env::time_now();

        let action1 = create_signed_user_add_action(
            &signing_key,
            owner,
            page.id(),
            serialized.clone(),
            nonce1,
        );
        assert!(MainInterface::apply_action(action1, &ApplyContext::empty()).is_ok());

        sleep(Duration::from_millis(2));

        let nonce2 = nonce1 - 1000;
        let action2 = create_signed_user_update_action(
            &signing_key,
            owner,
            page.id(),
            serialized,
            nonce2, // Lower nonce
            page.element().created_at(),
        );

        let result = MainInterface::apply_action(action2, &ApplyContext::empty());
        assert!(
            result.is_ok(),
            "stale-but-signed upsert must be silently skipped, got {result:?}"
        );
    }

    #[test]
    fn stale_upsert_with_invalid_signature_still_rejects() {
        // Security invariant: silent-skip on stale nonce applies
        // ONLY after the signature verifies. A stale upsert signed
        // by the wrong key must still reject as
        // `InvalidSignature` — without this, a future refactor that
        // moves the signature check after the nonce check would
        // silently accept unauthenticated stale traffic.
        env::reset_for_testing();

        let (signing_key, owner) = create_test_keypair();
        let (wrong_signing_key, _) = create_test_keypair();

        let mut element = Element::root();
        element.set_user_domain(owner);
        let page = Page::new_from_element("Page", element);
        let serialized = to_vec(&page).unwrap();

        let nonce1 = env::time_now();
        let action1 = create_signed_user_add_action(
            &signing_key,
            owner,
            page.id(),
            serialized.clone(),
            nonce1,
        );
        assert!(MainInterface::apply_action(action1, &ApplyContext::empty()).is_ok());

        sleep(Duration::from_millis(2));

        // Stale nonce + signed by WRONG key → InvalidSignature, not
        // silent-skip.
        let nonce2 = nonce1 - 1000;
        let action2 = create_signed_user_update_action(
            &wrong_signing_key,
            owner,
            page.id(),
            serialized,
            nonce2,
            page.element().created_at(),
        );

        let result = MainInterface::apply_action(action2, &ApplyContext::empty());
        assert!(
            matches!(result, Err(StorageError::InvalidSignature)),
            "stale upsert with invalid signature must reject as InvalidSignature, got {result:?}"
        );
    }

    #[test]
    fn sequential_updates_with_increasing_nonces_succeed() {
        crate::tests::common::register_test_merge_functions();
        env::reset_for_testing();

        let (signing_key, owner) = create_test_keypair();

        let mut element = Element::root();
        element.set_user_domain(owner);
        let mut page = Page::new_from_element("Version 1", element);
        let serialized = to_vec(&page).unwrap();

        let nonce1 = env::time_now();
        let action1 =
            create_signed_user_add_action(&signing_key, owner, page.id(), serialized, nonce1);
        assert!(MainInterface::apply_action(action1, &ApplyContext::empty()).is_ok());

        // Multiple updates with increasing nonces
        for i in 2..=5 {
            sleep(Duration::from_millis(2));
            page.title = format!("Version {}", i);
            let serialized = to_vec(&page).unwrap();
            let nonce = env::time_now();

            let action = create_signed_user_update_action(
                &signing_key,
                owner,
                page.id(),
                serialized,
                nonce,
                page.element().created_at(),
            );
            assert!(
                MainInterface::apply_action(action, &ApplyContext::empty()).is_ok(),
                "Update {} should succeed",
                i
            );
        }

        // Verify final state
        let retrieved = MainInterface::find_by_id::<Page>(page.id()).unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().title, "Version 5");
    }

    #[test]
    fn out_of_order_nonces_are_silently_skipped_for_upsert() {
        env::reset_for_testing();

        let (signing_key, owner) = create_test_keypair();

        let mut element = Element::root();
        element.set_user_domain(owner);
        let page = Page::new_from_element("Page", element);
        let serialized1 = to_vec(&page).unwrap();

        // Create first action - stores updated_at in index
        let first_nonce = env::time_now();
        let action_first = create_signed_user_add_action(
            &signing_key,
            owner,
            page.id(),
            serialized1.clone(),
            first_nonce,
        );
        assert!(MainInterface::apply_action(action_first, &ApplyContext::empty()).is_ok());

        sleep(Duration::from_millis(10));

        // Try action with nonce OLDER than stored updated_at — upsert
        // now silently skips (returns Ok(())) rather than rejecting
        // with `NonceReplay`. See the rationale on
        // `replay_with_lower_nonce_is_silent_noop_for_upsert` above
        // and the apply_action User arm comment. State must NOT be
        // downgraded — the stale action is dropped without
        // overwriting the newer stored value.
        let old_nonce = first_nonce - 1_000_000_000; // 1 second before first action
        let action_old = create_signed_user_update_action(
            &signing_key,
            owner,
            page.id(),
            serialized1,
            old_nonce,
            page.element().created_at(),
        );

        let result = MainInterface::apply_action(action_old, &ApplyContext::empty());
        assert!(
            result.is_ok(),
            "stale-but-signed upsert must be silently skipped, got {result:?}"
        );

        // Confirm the stored state was NOT downgraded by the stale
        // upsert. Stored nonce should still be at or above the
        // first action's nonce, not the stale `old_nonce`.
        let stored_nonce = <Index<MainStorage>>::get_metadata(page.id())
            .unwrap()
            .map(|m| *m.updated_at)
            .unwrap_or(0);
        assert!(
            stored_nonce >= first_nonce,
            "stale upsert must not downgrade stored nonce; stored={stored_nonce} \
             first={first_nonce} stale_attempt={old_nonce}"
        );
        assert!(
            stored_nonce > old_nonce,
            "stale upsert must not downgrade stored nonce; stored={stored_nonce} \
             stale_attempt={old_nonce}"
        );
    }
}

/// Tests for Shared storage replay protection.
///
/// Mirrors the User-arm tests in `user_storage_replay_protection` so the
/// silent-skip-on-stale contract is enforced consistently across both
/// signed storage types.
#[cfg(test)]
mod shared_storage_replay_protection {
    use std::collections::BTreeSet;
    use std::thread::sleep;
    use std::time::Duration;

    use ed25519_dalek::SigningKey;

    use crate::address::Id;
    use crate::env;
    use crate::index::Index;
    use crate::interface::{ApplyContext, MainInterface};
    use crate::store::MainStorage;
    use crate::tests::common::{build_signed_shared_action, pubkey_of, setup_root_for_main};

    fn make_signing_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    #[test]
    fn stale_shared_upsert_does_not_downgrade_state() {
        // Mirror of `out_of_order_nonces_are_silently_skipped_for_upsert`
        // for the Shared arm. A signature-verified action whose nonce
        // is below the locally stored nonce must return Ok(()) (silent
        // skip) AND must not downgrade the stored nonce.
        env::reset_for_testing();
        let root = setup_root_for_main();

        let alice_sk = make_signing_key(0xA1);
        let alice = pubkey_of(&alice_sk);
        let writers: BTreeSet<_> = [alice].into_iter().collect();
        let id = Id::new([0x5E; 32]);

        // Bootstrap with a fresh nonce.
        let nonce1 = env::time_now();
        let bootstrap = build_signed_shared_action(
            true,
            id,
            b"v0".to_vec(),
            writers.clone(),
            nonce1,
            &alice_sk,
            vec![root.clone()],
        );
        MainInterface::apply_action(bootstrap, &ApplyContext::empty()).unwrap();

        sleep(Duration::from_millis(2));

        // Stale-but-signed update (nonce < stored). Must Ok(()) without
        // downgrading state.
        let nonce_stale = nonce1.saturating_sub(1_000_000);
        let stale = build_signed_shared_action(
            false,
            id,
            b"v1".to_vec(),
            writers,
            nonce_stale,
            &alice_sk,
            vec![],
        );
        let result = MainInterface::apply_action(stale, &ApplyContext::empty());
        assert!(
            result.is_ok(),
            "stale-but-signed Shared upsert must be silently skipped, got {result:?}"
        );

        let stored_nonce = <Index<MainStorage>>::get_metadata(id)
            .unwrap()
            .map(|m| *m.updated_at)
            .unwrap_or(0);
        assert!(
            stored_nonce >= nonce1,
            "stale Shared upsert must not downgrade stored nonce; stored={stored_nonce} \
             bootstrap_nonce={nonce1} stale_attempt={nonce_stale}"
        );
        assert!(
            stored_nonce > nonce_stale,
            "stale Shared upsert must not downgrade stored nonce; stored={stored_nonce} \
             stale_attempt={nonce_stale}"
        );
    }
}

/// Tests for `SharedStorage` writer-set rotation authentication.
///
/// A writer-set rotation propagates as a signed per-entity action and is
/// verified at merge against the *current* writer set (resolved from the
/// rotation log / `effective_writers`, with the stored writers as the
/// fallback). A rotation forged by a non-writer must be rejected — this is the
/// merge-time backstop behind the local writer gate, and the property that
/// makes the writer set unforgeable.
#[cfg(test)]
mod shared_storage_rotation_authentication {
    use std::collections::BTreeSet;

    use ed25519_dalek::SigningKey;

    use crate::address::Id;
    use crate::entities::StorageType;
    use crate::env;
    use crate::index::Index;
    use crate::interface::{ApplyContext, MainInterface, StorageError};
    use crate::store::MainStorage;
    use crate::tests::common::{
        build_signed_member_action, build_signed_member_delete, build_signed_shared_action,
        pubkey_of, setup_root_for_main,
    };

    fn make_signing_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    #[test]
    fn forged_shared_rotation_rejected_at_merge() {
        env::reset_for_testing();
        let root = setup_root_for_main();

        let alice_sk = make_signing_key(0xA1);
        let alice = pubkey_of(&alice_sk);
        let mallory_sk = make_signing_key(0x4D); // a context member, NOT a writer
        let mallory = pubkey_of(&mallory_sk);

        let writers: BTreeSet<_> = [alice].into_iter().collect();
        let id = Id::new([0x5E; 32]);

        // Bootstrap the Shared entity with writers = {alice}, signed by alice.
        let nonce1 = env::time_now();
        let bootstrap = build_signed_shared_action(
            true,
            id,
            b"v0".to_vec(),
            writers.clone(),
            nonce1,
            &alice_sk,
            vec![root],
        );
        MainInterface::apply_action(bootstrap, &ApplyContext::empty()).unwrap();

        // Mallory forges a rotation: an Update that swaps the writer set to
        // {mallory}, signed by mallory (who is not a current writer). The
        // verifier resolves the authoritative writer set to {alice} (here via
        // effective_writers, as the node sync layer would from the rotation
        // log) and rejects mallory's signature.
        let forged = build_signed_shared_action(
            false,
            id,
            b"v0".to_vec(),
            [mallory].into_iter().collect(),
            nonce1 + 1_000_000,
            &mallory_sk,
            vec![],
        );
        let ctx = ApplyContext {
            effective_writers: Some(crate::entities::full_mask(writers.clone())),
            delta_id: None,
            delta_hlc: None,
        };
        let result = MainInterface::apply_action(forged, &ctx);
        assert!(
            matches!(result, Err(StorageError::InvalidSignature)),
            "forged rotation by a non-writer must be rejected, got {result:?}"
        );

        // The stored writer set is unchanged: the honest node still trusts only
        // {alice}.
        let stored = <Index<MainStorage>>::get_metadata(id).unwrap().unwrap();
        match stored.storage_type {
            StorageType::Shared { writers: w, .. } => {
                assert_eq!(
                    w,
                    crate::entities::full_mask(writers.clone()),
                    "writer set must be unchanged after forged rotation"
                );
            }
            other => panic!("expected Shared storage_type, got {other:?}"),
        }
    }

    #[test]
    fn forged_rotation_rejected_via_stored_writers_fallback() {
        // Complement to `forged_shared_rotation_rejected_at_merge`: exercise the
        // path where the apply context carries NO `effective_writers` (empty
        // ctx). The verifier must then fall back to the entity's *stored* writer
        // set and still reject a non-writer's forged rotation — covering the
        // case where rotation-log resolution yielded nothing.
        env::reset_for_testing();
        let root = setup_root_for_main();

        let alice_sk = make_signing_key(0xA1);
        let alice = pubkey_of(&alice_sk);
        let mallory_sk = make_signing_key(0x4D);
        let mallory = pubkey_of(&mallory_sk);

        let writers: BTreeSet<_> = [alice].into_iter().collect();
        let id = Id::new([0x5E; 32]);

        let nonce1 = env::time_now();
        let bootstrap = build_signed_shared_action(
            true,
            id,
            b"v0".to_vec(),
            writers.clone(),
            nonce1,
            &alice_sk,
            vec![root],
        );
        MainInterface::apply_action(bootstrap, &ApplyContext::empty()).unwrap();

        // Empty ctx → no effective_writers → verifier falls back to stored
        // writers {alice}; mallory's signature is not from a stored writer.
        let forged = build_signed_shared_action(
            false,
            id,
            b"v0".to_vec(),
            [mallory].into_iter().collect(),
            nonce1 + 1_000_000,
            &mallory_sk,
            vec![],
        );
        let result = MainInterface::apply_action(forged, &ApplyContext::empty());
        assert!(
            matches!(result, Err(StorageError::InvalidSignature)),
            "forged rotation must be rejected via the stored-writers fallback, got {result:?}"
        );

        let stored = <Index<MainStorage>>::get_metadata(id).unwrap().unwrap();
        match stored.storage_type {
            StorageType::Shared { writers: w, .. } => {
                assert_eq!(w, crate::entities::full_mask(writers.clone()))
            }
            other => panic!("expected Shared storage_type, got {other:?}"),
        }
    }

    #[test]
    fn authentic_rotation_by_current_writer_accepted() {
        env::reset_for_testing();
        let root = setup_root_for_main();

        let alice_sk = make_signing_key(0xA1);
        let alice = pubkey_of(&alice_sk);
        let bob_sk = make_signing_key(0xB0);
        let bob = pubkey_of(&bob_sk);

        let writers: BTreeSet<_> = [alice].into_iter().collect();
        let id = Id::new([0x5E; 32]);

        let nonce1 = env::time_now();
        let bootstrap = build_signed_shared_action(
            true,
            id,
            b"v0".to_vec(),
            writers.clone(),
            nonce1,
            &alice_sk,
            vec![root],
        );
        MainInterface::apply_action(bootstrap, &ApplyContext::empty()).unwrap();

        // Alice (a current writer) rotates the set to {alice, bob}. Verified
        // against the current set {alice}; alice's signature is valid.
        let new_writers: BTreeSet<_> = [alice, bob].into_iter().collect();
        let rotation = build_signed_shared_action(
            false,
            id,
            b"v0".to_vec(),
            new_writers.clone(),
            nonce1 + 1_000_000,
            &alice_sk,
            vec![],
        );
        // Populate delta_id/delta_hlc so the rotation-log write hook fires and we
        // can assert the rotation actually took effect (not just that it was
        // accepted). The writer set is persisted to the rotation log, not the
        // index `storage_type` (apply does not patch a child's own metadata) — so
        // the log is what we assert.
        use crate::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
        let delta_hlc = HybridTimestamp::new(Timestamp::new(
            NTP64(nonce1 + 1_000_000),
            ID::from(core::num::NonZeroU128::new(1).unwrap()),
        ));
        let ctx = ApplyContext {
            effective_writers: Some(crate::entities::full_mask(writers.clone())),
            delta_id: Some([0xD1; 32]),
            delta_hlc: Some(delta_hlc),
        };
        MainInterface::apply_action(rotation, &ctx)
            .expect("authentic rotation by a current writer must be accepted");

        // The rotation must be recorded in the wrapper's rotation log with the
        // new writer set.
        let log = crate::rotation_log::load::<MainStorage>(id)
            .unwrap()
            .expect("rotation log must exist after an accepted rotation");
        assert_eq!(
            log.entries.last().expect("a rotation entry").new_writers,
            crate::entities::full_mask(new_writers.clone()),
            "accepted rotation must record the new writer set in the rotation log"
        );
    }

    /// The headline property of the anchor design: rotating the anchor's writer
    /// set retroactively revokes write access to its members — *without*
    /// changing any member's bytes. A member written by Bob while he was a
    /// writer becomes un-writable by Bob the instant the anchor rotates him out,
    /// even though the member entity itself is byte-identical throughout.
    ///
    /// We model the rotation by the writer set the node's `writers_at` would
    /// resolve from the anchor's rotation log at the delta's causal cut, passed
    /// as `effective_writers`. The member carries only its anchor pointer, so
    /// the SAME stored member is verified against {alice, bob} before the
    /// rotation and {alice} after — no per-member re-stamp involved.
    #[test]
    fn rotating_anchor_retroactively_revokes_member_writes() {
        env::reset_for_testing();
        let root = setup_root_for_main();

        let alice_sk = make_signing_key(0xA1);
        let alice = pubkey_of(&alice_sk);
        let bob_sk = make_signing_key(0xB0);
        let bob = pubkey_of(&bob_sk);

        let anchor = Id::new([0xA0; 32]);
        let member = Id::new([0x3E; 32]);
        let pre: BTreeSet<_> = [alice, bob].into_iter().collect();
        let post: BTreeSet<_> = [alice].into_iter().collect();

        // Bootstrap the anchor (a `Shared` entity) with writers {alice, bob}.
        let n0 = env::time_now();
        let bootstrap = build_signed_shared_action(
            true,
            anchor,
            b"anchor".to_vec(),
            pre.clone(),
            n0,
            &alice_sk,
            vec![root.clone()],
        );
        MainInterface::apply_action(bootstrap, &ApplyContext::empty()).unwrap();

        let pre_ctx = || ApplyContext {
            effective_writers: Some(crate::entities::full_mask(pre.clone())),
            delta_id: None,
            delta_hlc: None,
        };
        let post_ctx = || ApplyContext {
            effective_writers: Some(crate::entities::full_mask(post.clone())),
            delta_id: None,
            delta_hlc: None,
        };

        // BEFORE rotation: Bob (a writer) writes the member — accepted.
        let bob_add = build_signed_member_action(
            true,
            member,
            anchor,
            b"by-bob".to_vec(),
            n0 + 1_000_000,
            &bob_sk,
            vec![root.clone()],
        );
        MainInterface::apply_action(bob_add, &pre_ctx())
            .expect("a writer's member write must be accepted before rotation");

        // The stored member is anchored — no inline writer set.
        let stored = <Index<MainStorage>>::get_metadata(member).unwrap().unwrap();
        assert!(
            matches!(stored.storage_type, StorageType::SharedMember { anchor: a, .. } if a == anchor),
            "member must be anchored to the wrapper, got {:?}",
            stored.storage_type
        );

        // AFTER rotation (anchor writers now {alice}): Bob writes the SAME member
        // again — must be rejected. The member entity never changed; only the
        // anchor-resolved writer set did. This is retroactive revocation.
        let bob_revoked = build_signed_member_action(
            false,
            member,
            anchor,
            b"by-bob-after-revoke".to_vec(),
            n0 + 2_000_000,
            &bob_sk,
            vec![],
        );
        let result = MainInterface::apply_action(bob_revoked, &post_ctx());
        assert!(
            matches!(result, Err(StorageError::InvalidSignature)),
            "a rotated-out writer's member write must be rejected, got {result:?}"
        );

        // Alice (still a writer) can write the member under the same post-rotation
        // set — proving the rejection is authorization, not a broken member path.
        let alice_write = build_signed_member_action(
            false,
            member,
            anchor,
            b"by-alice".to_vec(),
            n0 + 3_000_000,
            &alice_sk,
            vec![],
        );
        MainInterface::apply_action(alice_write, &post_ctx())
            .expect("a current writer's member write must still be accepted");
    }

    /// The delete path mirrors the upsert path: deleting a member is authorized
    /// against the anchor's writers (resolved from the anchor's settled local
    /// state), not an inline set. A non-writer's member delete is rejected; a
    /// writer's is accepted.
    #[test]
    fn member_delete_authorized_against_anchor_writers() {
        env::reset_for_testing();
        let root = setup_root_for_main();

        let alice_sk = make_signing_key(0xA1);
        let alice = pubkey_of(&alice_sk);
        let mallory_sk = make_signing_key(0x4D); // a context member, NOT a writer
        let _mallory = pubkey_of(&mallory_sk);

        let anchor = Id::new([0xA0; 32]);
        let member = Id::new([0x3E; 32]);
        let writers: BTreeSet<_> = [alice].into_iter().collect();

        let n0 = env::time_now();
        // Anchor (Shared {alice}) + a member written by alice.
        let bootstrap = build_signed_shared_action(
            true,
            anchor,
            b"anchor".to_vec(),
            writers.clone(),
            n0,
            &alice_sk,
            vec![root.clone()],
        );
        MainInterface::apply_action(bootstrap, &ApplyContext::empty()).unwrap();
        let member_add = build_signed_member_action(
            true,
            member,
            anchor,
            b"v".to_vec(),
            n0 + 1_000_000,
            &alice_sk,
            vec![root.clone()],
        );
        MainInterface::apply_action(member_add, &ApplyContext::empty())
            .expect("writer's member add must be accepted");

        // Mallory (non-writer) tries to delete the member → rejected.
        let forged_delete = build_signed_member_delete(member, anchor, &mallory_sk, n0 + 2_000_000);
        let result = MainInterface::apply_action(forged_delete, &ApplyContext::empty());
        assert!(
            matches!(result, Err(StorageError::InvalidSignature)),
            "a non-writer's member delete must be rejected, got {result:?}"
        );

        // Alice (a writer) deletes the member → accepted.
        let ok_delete = build_signed_member_delete(member, anchor, &alice_sk, n0 + 3_000_000);
        MainInterface::apply_action(ok_delete, &ApplyContext::empty())
            .expect("a writer's member delete must be accepted");
    }

    /// OpMask gate: a writer holding `WRITE` but not `DELETE` may update a
    /// member entry, but a (validly-signed) delete from that same writer is
    /// rejected at merge — "write but not delete", enforced at apply time.
    #[test]
    fn member_write_capability_allows_update_but_not_delete() {
        use crate::entities::OpMask;

        env::reset_for_testing();
        let root = setup_root_for_main();

        let alice_sk = make_signing_key(0xA1);
        let alice = pubkey_of(&alice_sk);
        let anchor = Id::new([0xA0; 32]);
        let member = Id::new([0x3E; 32]);
        let writers: BTreeSet<_> = [alice].into_iter().collect();
        let n0 = env::time_now();

        // Bootstrap anchor {alice} + a member written by alice.
        let bootstrap = build_signed_shared_action(
            true,
            anchor,
            b"anchor".to_vec(),
            writers.clone(),
            n0,
            &alice_sk,
            vec![root.clone()],
        );
        MainInterface::apply_action(bootstrap, &ApplyContext::empty()).unwrap();
        let member_add = build_signed_member_action(
            true,
            member,
            anchor,
            b"v".to_vec(),
            n0 + 1_000_000,
            &alice_sk,
            vec![root.clone()],
        );
        MainInterface::apply_action(member_add, &ApplyContext::empty()).unwrap();

        // Alice's resolved capability is WRITE-only (no DELETE).
        let write_only = |id, hlc| crate::interface::ApplyContext {
            effective_writers: Some([(alice, OpMask::WRITE)].into_iter().collect()),
            delta_id: id,
            delta_hlc: hlc,
        };

        // An update is permitted (WRITE ⊇ WRITE).
        let member_update = build_signed_member_action(
            false,
            member,
            anchor,
            b"v2".to_vec(),
            n0 + 2_000_000,
            &alice_sk,
            vec![root.clone()],
        );
        MainInterface::apply_action(member_update, &write_only(None, None))
            .expect("a WRITE-capable writer's update must be accepted");

        // The same writer's (validly-signed) delete is refused at the op-gate.
        let del = build_signed_member_delete(member, anchor, &alice_sk, n0 + 3_000_000);
        let result = MainInterface::apply_action(del, &write_only(None, None));
        assert!(
            matches!(result, Err(StorageError::ActionNotAllowed(_))),
            "a writer lacking DELETE must be refused at the op-gate, got {result:?}"
        );

        // With FULL capability, the delete is accepted.
        let full = crate::interface::ApplyContext {
            effective_writers: Some([(alice, OpMask::FULL)].into_iter().collect()),
            delta_id: None,
            delta_hlc: None,
        };
        let del2 = build_signed_member_delete(member, anchor, &alice_sk, n0 + 4_000_000);
        MainInterface::apply_action(del2, &full).expect("FULL writer's delete must be accepted");
    }

    /// Snapshot verification of a member resolves the anchor's writers (the
    /// snapshot path bypasses the delta pipeline, so it must independently reach
    /// the anchor). A writer-signed member verifies; a non-writer-signed one is
    /// rejected.
    #[test]
    fn snapshot_verify_member_resolves_anchor_writers() {
        env::reset_for_testing();
        let root = setup_root_for_main();

        let alice_sk = make_signing_key(0xA1);
        let alice = pubkey_of(&alice_sk);
        let mallory_sk = make_signing_key(0x4D);

        let anchor = Id::new([0xA0; 32]);
        let member = Id::new([0x3E; 32]);
        let writers: BTreeSet<_> = [alice].into_iter().collect();

        let n0 = env::time_now();
        // Bootstrap the anchor so `resolve_anchor_writers` finds {alice} locally.
        let bootstrap = build_signed_shared_action(
            true,
            anchor,
            b"anchor".to_vec(),
            writers,
            n0,
            &alice_sk,
            vec![root],
        );
        MainInterface::apply_action(bootstrap, &ApplyContext::empty()).unwrap();

        // A member action signed by a writer (alice) — extract its metadata and
        // verify as a snapshot leaf.
        let data = b"snapshot-leaf".to_vec();
        let by_writer = build_signed_member_action(
            true,
            member,
            anchor,
            data.clone(),
            n0 + 1_000_000,
            &alice_sk,
            vec![],
        );
        let writer_meta = match &by_writer {
            crate::action::Action::Add { metadata, .. } => metadata.clone(),
            _ => unreachable!(),
        };
        assert!(
            MainInterface::verify_snapshot_entity_signature(member, &data, &writer_meta).is_ok(),
            "a writer-signed member snapshot leaf must verify against the anchor's writers"
        );

        // The same leaf signed by a non-writer (mallory) must be rejected.
        let by_nonwriter = build_signed_member_action(
            true,
            member,
            anchor,
            data.clone(),
            n0 + 1_000_000,
            &mallory_sk,
            vec![],
        );
        let nonwriter_meta = match &by_nonwriter {
            crate::action::Action::Add { metadata, .. } => metadata.clone(),
            _ => unreachable!(),
        };
        assert!(
            matches!(
                MainInterface::verify_snapshot_entity_signature(member, &data, &nonwriter_meta),
                Err(StorageError::InvalidSignature)
            ),
            "a non-writer-signed member snapshot leaf must be rejected"
        );
    }
}

/// Tests for Frozen storage verification.
///
/// Frozen storage is immutable and content-addressed:
/// - Cannot be updated after creation
/// - Cannot be deleted
/// - Data structure must be valid (key must match hash of value)
#[cfg(test)]
mod frozen_storage_verification {
    use super::*;
    use crate::address::Id;
    use crate::env;

    /// Helper to create valid frozen data blob.
    /// Format: [key_hash (32 bytes)] + [value_bytes (N bytes)] + [element_id (32 bytes)]
    fn create_valid_frozen_blob(value: &[u8], element_id: Id) -> Vec<u8> {
        let key_hash: [u8; 32] = Sha256::digest(value).into();
        let mut blob = Vec::new();
        blob.extend_from_slice(&key_hash);
        blob.extend_from_slice(value);
        blob.extend_from_slice(element_id.as_bytes());
        blob
    }

    /// Helper to create invalid frozen data blob (wrong key hash).
    fn create_invalid_frozen_blob(value: &[u8], element_id: Id) -> Vec<u8> {
        let mut key_hash: [u8; 32] = Sha256::digest(value).into();
        key_hash[0] ^= 0xFF; // Corrupt the hash
        let mut blob = Vec::new();
        blob.extend_from_slice(&key_hash);
        blob.extend_from_slice(value);
        blob.extend_from_slice(element_id.as_bytes());
        blob
    }

    #[test]
    fn frozen_add_with_valid_content_addressing_succeeds() {
        env::reset_for_testing();

        // Use root ID so it's not an orphan
        let id = Id::root();
        let value = b"immutable content";
        let blob = create_valid_frozen_blob(value, id);
        let timestamp = env::time_now();

        let action = Action::Add {
            id,
            data: blob,
            ancestors: vec![],
            metadata: Metadata {
                created_at: timestamp,
                updated_at: timestamp.into(),
                storage_type: StorageType::Frozen,
                crdt_type: None,
                field_name: None,
                schema_version: None,
            },
        };

        assert!(MainInterface::apply_action(action, &ApplyContext::empty()).is_ok());

        // Verify it was stored
        let stored = MainInterface::get(id);
        assert!(stored.is_ok());
    }

    #[test]
    fn frozen_add_with_invalid_content_addressing_fails() {
        env::reset_for_testing();

        // Use root ID so it's not an orphan
        let id = Id::root();
        let value = b"immutable content";
        let blob = create_invalid_frozen_blob(value, id);
        let timestamp = env::time_now();

        let action = Action::Add {
            id,
            data: blob,
            ancestors: vec![],
            metadata: Metadata {
                created_at: timestamp,
                updated_at: timestamp.into(),
                storage_type: StorageType::Frozen,
                crdt_type: None,
                field_name: None,
                schema_version: None,
            },
        };

        let result = MainInterface::apply_action(action, &ApplyContext::empty());
        assert!(result.is_err());
        match result {
            Err(StorageError::InvalidData(msg)) => {
                assert!(
                    msg.contains("corruption") || msg.contains("hash"),
                    "Error should mention corruption or hash: {}",
                    msg
                );
            }
            other => panic!("Expected InvalidData error, got {:?}", other),
        }
    }

    #[test]
    fn frozen_update_is_rejected() {
        env::reset_for_testing();

        // Use root ID so it's not an orphan
        let id = Id::root();
        let value = b"immutable content";
        let blob = create_valid_frozen_blob(value, id);
        let timestamp = env::time_now();

        // First add the frozen data
        let add_action = Action::Add {
            id,
            data: blob.clone(),
            ancestors: vec![],
            metadata: Metadata {
                created_at: timestamp,
                updated_at: timestamp.into(),
                storage_type: StorageType::Frozen,
                crdt_type: None,
                field_name: None,
                schema_version: None,
            },
        };
        assert!(MainInterface::apply_action(add_action, &ApplyContext::empty()).is_ok());

        sleep(Duration::from_millis(2));

        // Try to update - should fail
        let new_value = b"modified content";
        let new_blob = create_valid_frozen_blob(new_value, id);
        let new_timestamp = env::time_now();

        let update_action = Action::Update {
            id,
            data: new_blob,
            ancestors: vec![],
            metadata: Metadata {
                created_at: timestamp,
                updated_at: new_timestamp.into(),
                storage_type: StorageType::Frozen,
                crdt_type: None,
                field_name: None,
                schema_version: None,
            },
        };

        let result = MainInterface::apply_action(update_action, &ApplyContext::empty());
        assert!(result.is_err());
        match result {
            Err(StorageError::ActionNotAllowed(msg)) => {
                assert!(
                    msg.contains("Frozen") && msg.contains("updated"),
                    "Error should mention Frozen and updated: {}",
                    msg
                );
            }
            other => panic!("Expected ActionNotAllowed error, got {:?}", other),
        }
    }

    #[test]
    fn frozen_delete_is_rejected() {
        env::reset_for_testing();

        // Use root ID so it's not an orphan
        let id = Id::root();
        let value = b"immutable content";
        let blob = create_valid_frozen_blob(value, id);
        let timestamp = env::time_now();

        // First add the frozen data
        let add_action = Action::Add {
            id,
            data: blob,
            ancestors: vec![],
            metadata: Metadata {
                created_at: timestamp,
                updated_at: timestamp.into(),
                storage_type: StorageType::Frozen,
                crdt_type: None,
                field_name: None,
                schema_version: None,
            },
        };
        assert!(MainInterface::apply_action(add_action, &ApplyContext::empty()).is_ok());

        sleep(Duration::from_millis(2));

        // Try to delete - should fail
        let delete_action = Action::DeleteRef {
            id,
            deleted_at: env::time_now(),
            metadata: Metadata::default(),
        };

        let result = MainInterface::apply_action(delete_action, &ApplyContext::empty());
        assert!(result.is_err());
        match result {
            Err(StorageError::ActionNotAllowed(msg)) => {
                assert!(
                    msg.contains("Frozen") && msg.contains("deleted"),
                    "Error should mention Frozen and deleted: {}",
                    msg
                );
            }
            other => panic!("Expected ActionNotAllowed error, got {:?}", other),
        }
    }

    #[test]
    fn frozen_blob_too_small_fails() {
        env::reset_for_testing();

        // Use root ID so it's not an orphan
        let id = Id::root();
        let timestamp = env::time_now();

        // Blob with only 32 bytes (key hash) - missing value and element_id
        let blob = vec![0u8; 32];

        let action = Action::Add {
            id,
            data: blob,
            ancestors: vec![],
            metadata: Metadata {
                created_at: timestamp,
                updated_at: timestamp.into(),
                storage_type: StorageType::Frozen,
                crdt_type: None,
                field_name: None,
                schema_version: None,
            },
        };

        let result = MainInterface::apply_action(action, &ApplyContext::empty());
        assert!(result.is_err());
        match result {
            Err(StorageError::InvalidData(msg)) => {
                assert!(
                    msg.contains("small") || msg.contains("size"),
                    "Error should mention size: {}",
                    msg
                );
            }
            other => panic!("Expected InvalidData error, got {:?}", other),
        }
    }

    #[test]
    fn frozen_blob_exactly_minimum_size_succeeds() {
        env::reset_for_testing();

        // Use root ID so it's not an orphan
        let id = Id::root();
        let timestamp = env::time_now();

        // Exactly 64 bytes (32 key_hash + 32 element_id) - no value bytes
        // The hash of empty value must match the key
        let empty_hash: [u8; 32] = Sha256::digest([]).into();
        let mut blob = Vec::new();
        blob.extend_from_slice(&empty_hash);
        blob.extend_from_slice(id.as_bytes());

        let action = Action::Add {
            id,
            data: blob,
            ancestors: vec![],
            metadata: Metadata {
                created_at: timestamp,
                updated_at: timestamp.into(),
                storage_type: StorageType::Frozen,
                crdt_type: None,
                field_name: None,
                schema_version: None,
            },
        };

        // This should succeed since the hash of empty [] matches
        assert!(MainInterface::apply_action(action, &ApplyContext::empty()).is_ok());
    }
}

/// Tests for timestamp verification (drift protection).
///
/// Actions with timestamps too far in the future are rejected
/// to prevent LWW (Last-Write-Wins) time drift attacks.
#[cfg(test)]
mod timestamp_drift_protection {
    use super::*;
    use crate::env;

    #[test]
    fn action_with_future_timestamp_beyond_tolerance_fails() {
        env::reset_for_testing();

        let now = env::time_now();
        // Timestamp way in the future (beyond drift tolerance)
        let future_timestamp = now + DRIFT_TOLERANCE_NANOS + 1_000_000_000; // 6+ seconds ahead

        let page = Page::new_from_element("Future Page", Element::root());
        let serialized = to_vec(&page).unwrap();

        let action = Action::Add {
            id: page.id(),
            data: serialized,
            ancestors: vec![],
            metadata: Metadata {
                created_at: future_timestamp,
                updated_at: future_timestamp.into(),
                storage_type: StorageType::Public,
                crdt_type: None,
                field_name: None,
                schema_version: None,
            },
        };

        let result = MainInterface::apply_action(action, &ApplyContext::empty());
        assert!(result.is_err());
        match result {
            Err(StorageError::InvalidTimestamp(ts, local)) => {
                assert!(ts > local + DRIFT_TOLERANCE_NANOS);
            }
            other => panic!("Expected InvalidTimestamp error, got {:?}", other),
        }
    }

    #[test]
    fn action_with_future_timestamp_within_tolerance_succeeds() {
        env::reset_for_testing();

        let now = env::time_now();
        // Timestamp slightly in the future (within drift tolerance)
        let future_timestamp = now + DRIFT_TOLERANCE_NANOS - 1_000_000_000; // 4 seconds ahead

        let page = Page::new_from_element("Near Future Page", Element::root());
        let serialized = to_vec(&page).unwrap();

        let action = Action::Add {
            id: page.id(),
            data: serialized,
            ancestors: vec![],
            metadata: Metadata {
                created_at: future_timestamp,
                updated_at: future_timestamp.into(),
                storage_type: StorageType::Public,
                crdt_type: None,
                field_name: None,
                schema_version: None,
            },
        };

        // Should succeed since within tolerance
        assert!(MainInterface::apply_action(action, &ApplyContext::empty()).is_ok());
    }

    #[test]
    fn action_with_past_timestamp_succeeds() {
        env::reset_for_testing();

        let now = env::time_now();
        // Timestamp in the past
        let past_timestamp = now - 60_000_000_000; // 60 seconds ago

        let page = Page::new_from_element("Past Page", Element::root());
        let serialized = to_vec(&page).unwrap();

        let action = Action::Add {
            id: page.id(),
            data: serialized,
            ancestors: vec![],
            metadata: Metadata {
                created_at: past_timestamp,
                updated_at: past_timestamp.into(),
                storage_type: StorageType::Public,
                crdt_type: None,
                field_name: None,
                schema_version: None,
            },
        };

        // Past timestamps are fine
        assert!(MainInterface::apply_action(action, &ApplyContext::empty()).is_ok());
    }

    #[test]
    fn delete_ref_with_future_timestamp_beyond_tolerance_fails() {
        env::reset_for_testing();

        // First create an entity
        let mut page = Page::new_from_element("Page", Element::root());
        assert!(MainInterface::save(&mut page).unwrap());

        let now = env::time_now();
        let future_timestamp = now + DRIFT_TOLERANCE_NANOS + 1_000_000_000;

        let action = Action::DeleteRef {
            id: page.id(),
            deleted_at: future_timestamp,
            metadata: Metadata::default(),
        };

        let result = MainInterface::apply_action(action, &ApplyContext::empty());
        assert!(result.is_err());
        match result {
            Err(StorageError::InvalidTimestamp(_, _)) => {}
            other => panic!("Expected InvalidTimestamp error, got {:?}", other),
        }
    }
}

/// Tests for edge cases in storage type handling.
///
/// Verifies correct handling of:
/// - Owner mismatch for User storage
/// - Storage type change attempts
/// - Delete actions for User storage
#[cfg(test)]
mod storage_type_edge_cases {
    use calimero_primitives::identity::PublicKey;
    use ed25519_dalek::SigningKey;

    use super::*;
    use crate::address::Id;
    use crate::env;
    use crate::tests::common::{
        create_signed_user_add_action, create_signed_user_update_action, create_test_keypair,
        sign_action,
    };

    fn create_signed_delete_action(
        signing_key: &SigningKey,
        owner: PublicKey,
        id: Id,
        nonce: u64,
    ) -> Action {
        let deleted_at = env::time_now();

        let metadata = Metadata {
            created_at: 0, // Not used for delete
            updated_at: deleted_at.into(),
            storage_type: StorageType::User {
                owner,
                signature_data: Some(SignatureData {
                    signature: [0; 64],
                    nonce,
                    signer: None,
                }),
            },
            crdt_type: None,
            field_name: None,
            schema_version: None,
        };

        let mut action = Action::DeleteRef {
            id,
            deleted_at,
            metadata,
        };

        let signature = sign_action(&action, signing_key);

        if let Action::DeleteRef {
            ref mut metadata, ..
        } = action
        {
            if let StorageType::User {
                ref mut signature_data,
                ..
            } = metadata.storage_type
            {
                *signature_data = Some(SignatureData {
                    signature,
                    nonce,
                    signer: None,
                });
            }
        }

        action
    }

    #[test]
    fn user_update_with_different_owner_fails() {
        env::reset_for_testing();

        let (signing_key1, owner1) = create_test_keypair();
        let (signing_key2, owner2) = create_test_keypair();

        // Create entity owned by owner1
        let mut element = Element::root();
        element.set_user_domain(owner1);
        let page = Page::new_from_element("Page", element);
        let serialized = to_vec(&page).unwrap();

        let nonce1 = env::time_now();
        let action1 = create_signed_user_add_action(
            &signing_key1,
            owner1,
            page.id(),
            serialized.clone(),
            nonce1,
        );
        assert!(MainInterface::apply_action(action1, &ApplyContext::empty()).is_ok());

        sleep(Duration::from_millis(2));

        // Try to update with different owner - should fail
        let nonce2 = env::time_now();
        let action2 = create_signed_user_update_action(
            &signing_key2,
            owner2, // Different owner!
            page.id(),
            serialized,
            nonce2,
            page.element().created_at(),
        );

        let result = MainInterface::apply_action(action2, &ApplyContext::empty());
        assert!(result.is_err());
        match result {
            Err(StorageError::ActionNotAllowed(msg)) => {
                assert!(msg.contains("owner"), "Error should mention owner: {}", msg);
            }
            other => panic!("Expected ActionNotAllowed error, got {:?}", other),
        }
    }

    #[test]
    fn user_delete_with_valid_signature_succeeds() {
        env::reset_for_testing();

        let (signing_key, owner) = create_test_keypair();

        // Create user-owned entity
        let mut element = Element::root();
        element.set_user_domain(owner);
        let page = Page::new_from_element("Page", element);
        let serialized = to_vec(&page).unwrap();

        let nonce1 = env::time_now();
        let action1 =
            create_signed_user_add_action(&signing_key, owner, page.id(), serialized, nonce1);
        assert!(MainInterface::apply_action(action1, &ApplyContext::empty()).is_ok());

        sleep(Duration::from_millis(2));

        // Delete with valid signature
        let nonce2 = env::time_now();
        let delete_action = create_signed_delete_action(&signing_key, owner, page.id(), nonce2);

        assert!(MainInterface::apply_action(delete_action, &ApplyContext::empty()).is_ok());

        // Verify entity is deleted
        let retrieved = MainInterface::find_by_id::<Page>(page.id()).unwrap();
        assert!(retrieved.is_none());
    }

    #[test]
    fn user_delete_with_wrong_owner_fails() {
        env::reset_for_testing();

        let (signing_key1, owner1) = create_test_keypair();
        let (signing_key2, owner2) = create_test_keypair();

        // Create entity owned by owner1
        let mut element = Element::root();
        element.set_user_domain(owner1);
        let page = Page::new_from_element("Page", element);
        let serialized = to_vec(&page).unwrap();

        let nonce1 = env::time_now();
        let action1 =
            create_signed_user_add_action(&signing_key1, owner1, page.id(), serialized, nonce1);
        assert!(MainInterface::apply_action(action1, &ApplyContext::empty()).is_ok());

        sleep(Duration::from_millis(2));

        // Try to delete with different owner's signature
        let nonce2 = env::time_now();
        let delete_action = create_signed_delete_action(&signing_key2, owner2, page.id(), nonce2);

        let result = MainInterface::apply_action(delete_action, &ApplyContext::empty());
        assert!(result.is_err());
        match result {
            Err(StorageError::InvalidSignature) => {}
            other => panic!("Expected InvalidSignature error, got {:?}", other),
        }
    }

    #[test]
    fn user_delete_without_signature_fails() {
        env::reset_for_testing();

        let (signing_key, owner) = create_test_keypair();

        // Create user-owned entity
        let mut element = Element::root();
        element.set_user_domain(owner);
        let page = Page::new_from_element("Page", element);
        let serialized = to_vec(&page).unwrap();

        let nonce1 = env::time_now();
        let action1 =
            create_signed_user_add_action(&signing_key, owner, page.id(), serialized, nonce1);
        assert!(MainInterface::apply_action(action1, &ApplyContext::empty()).is_ok());

        sleep(Duration::from_millis(2));

        // Try to delete without signature
        let delete_action = Action::DeleteRef {
            id: page.id(),
            deleted_at: env::time_now(),
            metadata: Metadata {
                created_at: 0,
                updated_at: env::time_now().into(),
                storage_type: StorageType::User {
                    owner,
                    signature_data: None, // No signature!
                },
                crdt_type: None,
                field_name: None,
                schema_version: None,
            },
        };

        let result = MainInterface::apply_action(delete_action, &ApplyContext::empty());
        assert!(result.is_err());
        match result {
            Err(StorageError::InvalidData(msg)) => {
                assert!(
                    msg.contains("signed"),
                    "Error should mention signing: {}",
                    msg
                );
            }
            other => panic!("Expected InvalidData error, got {:?}", other),
        }
    }

    #[test]
    fn cannot_change_public_to_user_storage() {
        env::reset_for_testing();

        // Create public entity first
        let mut page = Page::new_from_element("Public Page", Element::root());
        assert!(MainInterface::save(&mut page).unwrap());

        sleep(Duration::from_millis(2));

        let (signing_key, owner) = create_test_keypair();

        // Try to update to User storage - should fail
        let nonce = env::time_now();
        let action = create_signed_user_update_action(
            &signing_key,
            owner,
            page.id(),
            to_vec(&page).unwrap(),
            nonce,
            page.element().created_at(),
        );

        let result = MainInterface::apply_action(action, &ApplyContext::empty());
        assert!(result.is_err());
        match result {
            Err(StorageError::ActionNotAllowed(msg)) => {
                assert!(
                    msg.contains("StorageType"),
                    "Error should mention StorageType: {}",
                    msg
                );
            }
            other => panic!("Expected ActionNotAllowed error, got {:?}", other),
        }
    }

    #[test]
    fn cannot_change_user_to_public_storage() {
        env::reset_for_testing();

        let (signing_key, owner) = create_test_keypair();

        // Create user-owned entity
        let mut element = Element::root();
        element.set_user_domain(owner);
        let page = Page::new_from_element("User Page", element);
        let serialized = to_vec(&page).unwrap();

        let nonce = env::time_now();
        let action1 = create_signed_user_add_action(
            &signing_key,
            owner,
            page.id(),
            serialized.clone(),
            nonce,
        );
        assert!(MainInterface::apply_action(action1, &ApplyContext::empty()).is_ok());

        sleep(Duration::from_millis(2));

        // Try to update to Public storage - should fail
        let timestamp = env::time_now();
        let action2 = Action::Update {
            id: page.id(),
            data: serialized,
            ancestors: vec![],
            metadata: Metadata {
                created_at: page.element().created_at(),
                updated_at: timestamp.into(),
                storage_type: StorageType::Public, // Changed to Public!
                crdt_type: None,
                field_name: None,
                schema_version: None,
            },
        };

        let result = MainInterface::apply_action(action2, &ApplyContext::empty());
        assert!(result.is_err());
        match result {
            Err(StorageError::ActionNotAllowed(msg)) => {
                assert!(
                    msg.contains("StorageType"),
                    "Error should mention StorageType: {}",
                    msg
                );
            }
            other => panic!("Expected ActionNotAllowed error, got {:?}", other),
        }
    }

    #[test]
    fn user_delete_replay_protection() {
        crate::tests::common::register_test_merge_functions();
        env::reset_for_testing();

        let (signing_key, owner) = create_test_keypair();

        // Create user-owned entity
        let mut element = Element::root();
        element.set_user_domain(owner);
        let page = Page::new_from_element("Page", element);
        let serialized = to_vec(&page).unwrap();

        let nonce1 = env::time_now();
        let action1 = create_signed_user_add_action(
            &signing_key,
            owner,
            page.id(),
            serialized.clone(),
            nonce1,
        );
        assert!(MainInterface::apply_action(action1, &ApplyContext::empty()).is_ok());

        sleep(Duration::from_millis(2));

        // Update entity (increases the nonce)
        let nonce2 = env::time_now();
        let action2 = create_signed_user_update_action(
            &signing_key,
            owner,
            page.id(),
            serialized,
            nonce2,
            page.element().created_at(),
        );
        assert!(MainInterface::apply_action(action2, &ApplyContext::empty()).is_ok());

        sleep(Duration::from_millis(2));

        // Try to delete with old nonce (replay attack)
        let delete_action = create_signed_delete_action(&signing_key, owner, page.id(), nonce1);

        let result = MainInterface::apply_action(delete_action, &ApplyContext::empty());
        assert!(result.is_err());
        match result {
            Err(StorageError::NonceReplay(_)) => {}
            other => panic!("Expected NonceReplay error, got {:?}", other),
        }

        // Entity should still exist
        let retrieved = MainInterface::find_by_id::<Page>(page.id()).unwrap();
        assert!(retrieved.is_some());
    }
}

/// PR-6c task 6c.3 — owner-driven convert at write time.
///
/// The owner's next ordinary signed write of a stale identity-gated entry must
/// re-stamp `Metadata.schema_version` to the binary's current target (read
/// type-erased via `calimero_sdk::app::schema_version()`) on the normal
/// monotonic-nonce write path — NOT under `with_merge_mode` (which bypasses the
/// replay-nonce check) and NOT entropy-suppressed. A non-owner can never drive
/// the convert: the local-owner stamp branch in `save_raw` only fires when the
/// executor is the owner.
#[cfg(test)]
mod owner_driven_convert {
    use borsh::{BorshDeserialize, BorshSerialize};
    use calimero_sdk::event::NoEvent;
    use calimero_sdk::state::{AppState, AppStateInit};

    use super::*;
    use crate::env;
    use crate::tests::common::{create_signed_user_add_action, create_test_keypair};

    // A v2 binary that declares its target schema version. `register_schema_version`
    // surfaces this through the process-global atomic that `save_raw` reads.
    #[derive(BorshSerialize, BorshDeserialize)]
    struct V2;
    impl AppStateInit for V2 {
        type Return = V2;
    }
    impl AppState for V2 {
        type Event<'a> = NoEvent;
        const SCHEMA_VERSION: u32 = 1;
    }

    // A legacy binary that never declared a target — surfaces the unversioned 0.
    #[derive(BorshSerialize, BorshDeserialize)]
    struct Unversioned;
    impl AppStateInit for Unversioned {
        type Return = Unversioned;
    }
    impl AppState for Unversioned {
        type Event<'a> = NoEvent;
    }

    /// Seed a `User` entry owned by `owner` (schema_version `None`, the legacy
    /// unmarked shape) as a child of the root, returning its `Id`.
    fn seed_stale_user_entry(signing_key: &ed25519_dalek::SigningKey, owner: PublicKey) -> Id {
        let root = crate::tests::common::setup_root_for_main();

        // A non-root child so its id is distinct from the registered Public root.
        let mut element = Element::new(None);
        element.set_user_domain(owner);
        let page = Page::new_from_element("v1", element);
        let id = page.id();
        let serialized = borsh::to_vec(&page).unwrap();

        let nonce = env::time_now();
        let mut action = create_signed_user_add_action(signing_key, owner, id, serialized, nonce);
        // Re-parent the add under the registered root so it is not an orphan.
        if let Action::Add {
            ref mut ancestors, ..
        } = action
        {
            *ancestors = vec![root];
        }
        // Re-sign after mutating ancestors so the signature stays valid.
        if let Action::Add {
            ref mut metadata, ..
        } = action
        {
            if let StorageType::User {
                signature_data: Some(ref mut sd),
                ..
            } = metadata.storage_type
            {
                sd.signature = [0; 64];
            }
        }
        let payload = action.payload_for_signing();
        let signature = ed25519_dalek::Signer::sign(signing_key, &payload).to_bytes();
        if let Action::Add {
            ref mut metadata, ..
        } = action
        {
            if let StorageType::User {
                signature_data: Some(ref mut sd),
                ..
            } = metadata.storage_type
            {
                sd.signature = signature;
            }
        }

        MainInterface::apply_action(action, &ApplyContext::empty()).expect("seed user entry");

        // Sanity: stored entry is the legacy unmarked shape.
        let stored = Index::<MainStorage>::get_metadata(id).unwrap().unwrap();
        assert_eq!(stored.schema_version, None, "seeded entry must be unmarked");
        id
    }

    #[test]
    #[serial_test::serial]
    fn owner_write_stamps_target_schema_and_advances_nonce() {
        env::reset_for_testing();
        calimero_sdk::app::register_schema_version::<V2>();

        let (signing_key, owner) = create_test_keypair();
        let id = seed_stale_user_entry(&signing_key, owner);

        let stored = Index::<MainStorage>::get_metadata(id).unwrap().unwrap();
        let prior_nonce = stored.updated_at();
        let new_nonce = prior_nonce + 1_000_000;

        // The owner's next ordinary write: same owner, new value bytes, strictly
        // greater `updated_at`. `save_raw`'s local-owner branch only fires when
        // the executor is the owner, so drive it under the owner's identity.
        let convert_meta = Metadata {
            created_at: stored.created_at(),
            updated_at: new_nonce.into(),
            storage_type: StorageType::User {
                owner,
                signature_data: None,
            },
            crdt_type: None,
            field_name: None,
            schema_version: None,
        };
        env::with_executor_id(*owner, || {
            assert!(!env::in_merge_mode(), "convert must run on the normal path");
            MainInterface::save_raw(id, b"v2-bytes".to_vec(), convert_meta)
                .expect("owner convert write");
        });

        let m = Index::<MainStorage>::get_metadata(id).unwrap().unwrap();
        assert_eq!(
            m.schema_version,
            Some(1),
            "owner write stamps the binary's target schema version"
        );
        assert_eq!(
            m.updated_at(),
            new_nonce,
            "nonce strictly advanced (not merge-mode/entropy-suppressed)"
        );
        // The owner is unchanged — the convert re-stamps value + schema, never owner.
        match m.storage_type {
            StorageType::User { owner: o, .. } => assert_eq!(o, owner, "owner unchanged"),
            other => panic!("expected User storage_type, got {other:?}"),
        }

        // Reset the process-global so a parallel/later test sees the default.
        calimero_sdk::app::register_schema_version::<Unversioned>();
    }

    #[test]
    #[serial_test::serial]
    fn non_owner_write_does_not_convert() {
        env::reset_for_testing();
        calimero_sdk::app::register_schema_version::<V2>();

        let (signing_key, owner) = create_test_keypair();
        let (_, not_owner) = create_test_keypair();
        let id = seed_stale_user_entry(&signing_key, owner);

        let stored = Index::<MainStorage>::get_metadata(id).unwrap().unwrap();
        let new_nonce = stored.updated_at() + 1_000_000;

        let convert_meta = Metadata {
            created_at: stored.created_at(),
            updated_at: new_nonce.into(),
            storage_type: StorageType::User {
                owner,
                signature_data: None,
            },
            crdt_type: None,
            field_name: None,
            schema_version: None,
        };
        // Executor is NOT the owner: the local-owner stamp branch must not fire,
        // so the entry is never converted (schema stays None, no owner signature).
        env::with_executor_id(*not_owner, || {
            MainInterface::save_raw(id, b"v2-bytes".to_vec(), convert_meta)
                .expect("save_raw runs but does not stamp owner");
        });

        let m = Index::<MainStorage>::get_metadata(id).unwrap().unwrap();
        assert_eq!(
            m.schema_version, None,
            "a non-owner write must NOT convert the entry (no owner stamp)"
        );

        calimero_sdk::app::register_schema_version::<Unversioned>();
    }

    #[test]
    #[serial_test::serial]
    fn convert_does_not_run_in_merge_mode() {
        env::reset_for_testing();
        calimero_sdk::app::register_schema_version::<V2>();

        let (signing_key, owner) = create_test_keypair();
        let id = seed_stale_user_entry(&signing_key, owner);
        let stored = Index::<MainStorage>::get_metadata(id).unwrap().unwrap();
        let new_nonce = stored.updated_at() + 1_000_000;

        let convert_meta = Metadata {
            created_at: stored.created_at(),
            updated_at: new_nonce.into(),
            storage_type: StorageType::User {
                owner,
                signature_data: None,
            },
            crdt_type: None,
            field_name: None,
            schema_version: None,
        };
        // Guard O4: even an owner-keyed write must NOT drive the convert when it
        // runs inside a merge scope. Merge mode bypasses the replay-nonce check
        // (interface.rs:916), so a convert stamped here would re-shape an
        // identity-gated entry on the idempotent merge re-apply path rather than
        // as a fresh, monotonic, owner-signed delta — exactly the security
        // hazard PR-6c must avoid. Drive the owner write under
        // `with_merge_mode` and assert the entry stays unconverted.
        let out = env::with_executor_id(*owner, || {
            env::with_merge_mode(|| MainInterface::save_raw(id, b"v2".to_vec(), convert_meta))
        });
        assert!(out.is_ok(), "the write itself still succeeds in merge mode");
        assert!(
            !env::in_merge_mode(),
            "merge mode must be restored after the write"
        );

        let m = Index::<MainStorage>::get_metadata(id).unwrap().unwrap();
        assert_eq!(
            m.schema_version, None,
            "the owner-driven convert must be suppressed under merge mode \
             (schema_version must stay unstamped)"
        );

        calimero_sdk::app::register_schema_version::<Unversioned>();
    }

    /// Seed a `User` entry owned by `owner` (schema `None`) under the root of an
    /// arbitrary replica `Interface<S>`, returning its `Id` and the root
    /// `ChildInfo` (for building a subsequent signed `Action::Update`).
    fn seed_user_entry_on<S: crate::store::StorageAdaptor>(
        signing_key: &ed25519_dalek::SigningKey,
        owner: PublicKey,
    ) -> (Id, crate::entities::ChildInfo) {
        let root_id = Id::root();
        let root_meta = Metadata::default();
        Index::<S>::add_root(crate::entities::ChildInfo::new(
            root_id,
            [0; 32],
            root_meta.clone(),
        ))
        .unwrap();
        let (root_full, _) = Index::<S>::get_hashes_for(root_id).unwrap().unwrap();
        let root = crate::entities::ChildInfo::new(root_id, root_full, root_meta);

        let mut element = Element::new(None);
        element.set_user_domain(owner);
        let page = Page::new_from_element("v1", element);
        let id = page.id();
        let serialized = borsh::to_vec(&page).unwrap();

        let nonce = env::time_now();
        let mut metadata = Metadata {
            created_at: nonce,
            updated_at: nonce.into(),
            storage_type: StorageType::User {
                owner,
                signature_data: Some(SignatureData {
                    signature: [0; 64],
                    nonce,
                    signer: None,
                }),
            },
            crdt_type: None,
            field_name: None,
            schema_version: None,
        };
        let mut action = Action::Add {
            id,
            data: serialized,
            ancestors: vec![root.clone()],
            metadata: metadata.clone(),
        };
        let payload = action.payload_for_signing();
        let signature = ed25519_dalek::Signer::sign(signing_key, &payload).to_bytes();
        if let StorageType::User {
            signature_data: Some(ref mut sd),
            ..
        } = metadata.storage_type
        {
            sd.signature = signature;
        }
        if let Action::Add {
            metadata: ref mut m,
            ..
        } = action
        {
            *m = metadata;
        }
        Interface::<S>::apply_action(action, &ApplyContext::empty()).expect("seed user on replica");
        (id, root)
    }

    // Two-identity convergence: identity A (owner) converts an entry; the
    // resulting ordinary signed `Action::Update` replicates to identity B, whose
    // stored entry must then show `schema_version = Some(target)` — proving the
    // convert lands on a remote as a normal signed delta (NOT merge-mode, NOT
    // byte-identity), via the normal `new_nonce > last_nonce` apply branch.
    #[test]
    #[serial_test::serial]
    fn convert_replicates_to_a_second_identity() {
        type ReplicaB = MockedStorage<7301>;
        type InterfaceB = Interface<ReplicaB>;

        env::reset_for_testing();
        calimero_sdk::app::register_schema_version::<V2>();

        let (owner_sk, owner) = create_test_keypair();

        // Replica B starts with the legacy unmarked entry (the pre-convert shape).
        let (id, root) = seed_user_entry_on::<ReplicaB>(&owner_sk, owner);
        let before = Index::<ReplicaB>::get_metadata(id).unwrap().unwrap();
        assert_eq!(before.schema_version, None, "B starts unconverted");

        // The owner's convert, as it replicates: an ordinary signed
        // `Action::Update` with new value bytes, a strictly-greater nonce, and
        // the new schema tag. The signature commits to (id, data, owner, nonce)
        // — NOT to schema_version (Merkle/metadata-invisible) — so it verifies
        // normally on B.
        let new_nonce = before.updated_at() + 1_000_000;
        let mut metadata = Metadata {
            created_at: before.created_at(),
            updated_at: new_nonce.into(),
            storage_type: StorageType::User {
                owner,
                signature_data: Some(SignatureData {
                    signature: [0; 64],
                    nonce: new_nonce,
                    signer: None,
                }),
            },
            crdt_type: None,
            field_name: None,
            schema_version: Some(1),
        };
        let mut update = Action::Update {
            id,
            data: b"v2-bytes".to_vec(),
            ancestors: vec![root],
            metadata: metadata.clone(),
        };
        let payload = update.payload_for_signing();
        let signature = ed25519_dalek::Signer::sign(&owner_sk, &payload).to_bytes();
        if let StorageType::User {
            signature_data: Some(ref mut sd),
            ..
        } = metadata.storage_type
        {
            sd.signature = signature;
        }
        if let Action::Update {
            metadata: ref mut m,
            ..
        } = update
        {
            *m = metadata;
        }

        // B applies the replicated convert via the normal apply path.
        InterfaceB::apply_action(update, &ApplyContext::empty()).expect("B applies convert delta");

        let after = Index::<ReplicaB>::get_metadata(id).unwrap().unwrap();
        assert_eq!(
            after.schema_version,
            Some(1),
            "the replicated convert advances B's stored schema tag"
        );
        assert_eq!(
            after.updated_at(),
            new_nonce,
            "B's entry advanced on the normal monotonic-nonce branch"
        );

        calimero_sdk::app::register_schema_version::<Unversioned>();
    }

    /// P3 (core#2716): the rotation log stored as a hashed child entity
    /// (`crdt_type: RotationLog`) UNIONS divergent saves rather than LWW-
    /// overwriting — proving the relocated log converges on ordinary sync. This
    /// exercises the whole foundation end to end: `save_rotation_log_child` →
    /// `save_raw` → `save_internal`'s always-union dispatch → `merge_rotation_log`.
    #[test]
    fn rotation_log_child_unions_divergent_saves() {
        use core::num::NonZeroU128;
        use std::collections::BTreeMap;

        use calimero_primitives::identity::PublicKey;

        use crate::address::Id;
        use crate::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
        use crate::rotation_log::{RotationLog, RotationLogEntry};

        type S = MockedStorage<7411>;
        let anchor = Id::new([0x55; 32]);

        // SIGNED entries: `append_rotation_to_child` skips unsigned
        // (`signer == None`) entries — they carry no authoritative writer-set
        // fact and would diverge the collection hash across nodes (only the
        // originator self-logs the unsigned bootstrap). This test exercises the
        // collection's UNION behaviour, so its entries must be signed to land.
        let entry = |d: u8| RotationLogEntry {
            delta_id: [d; 32],
            delta_hlc: HybridTimestamp::new(Timestamp::new(
                NTP64(u64::from(d)),
                ID::from(NonZeroU128::new(1).unwrap()),
            )),
            signer: Some(PublicKey::from([d; 32])),
            signature: Some([d; 64]),
            signed_payload: Some([d; 32]),
            new_writers: BTreeMap::new(),
            writers_nonce: u64::from(d),
        };

        // Local log {1,2} → child entity.
        let a = RotationLog {
            snapshot: None,
            entries: vec![entry(1), entry(2)],
        };
        Interface::<S>::save_rotation_log_child(anchor, &a).expect("save A");
        let loaded = Interface::<S>::load_rotation_log_child(anchor).expect("load A");
        let ids: Vec<u8> = loaded.entries.iter().map(|e| e.delta_id[0]).collect();
        assert_eq!(ids, vec![1, 2], "child holds the saved log");

        // A divergent peer log {2,3} merges in — must UNION to {1,2,3}, NOT
        // LWW-overwrite to {2,3} (which is what the old timestamp branches did).
        let b = RotationLog {
            snapshot: None,
            entries: vec![entry(2), entry(3)],
        };
        Interface::<S>::save_rotation_log_child(anchor, &b).expect("merge B");
        let merged = Interface::<S>::load_rotation_log_child(anchor).expect("load merged");
        let mut ids: Vec<u8> = merged.entries.iter().map(|e| e.delta_id[0]).collect();
        ids.sort_unstable();
        assert_eq!(
            ids,
            vec![1, 2, 3],
            "divergent save must union (always-union dispatch), not LWW-overwrite"
        );
    }
}
