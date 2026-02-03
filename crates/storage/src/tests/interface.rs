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
            metadata: page.element().metadata.clone(),
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
            metadata: page.element().metadata.clone(),
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
            metadata: Metadata::default(),
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
            metadata: Metadata::default(),
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

        assert!(MainInterface::apply_action(old_delete).is_ok());

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
            metadata: page.element().metadata.clone(),
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
    use calimero_primitives::identity::PublicKey;
    use ed25519_dalek::{Signer, SigningKey};

    use super::*;
    use crate::address::Id;
    use crate::env;

    /// Helper to create a test keypair and public key
    fn create_test_keypair() -> (SigningKey, PublicKey) {
        let mut seed = [0u8; 32];
        env::random_bytes(&mut seed);
        let signing_key = SigningKey::from_bytes(&seed);
        let verifying_key = signing_key.verifying_key();
        let public_key = PublicKey::from(*verifying_key.as_bytes());
        (signing_key, public_key)
    }

    /// Helper to sign an action with the given signing key
    fn sign_action(action: &Action, signing_key: &SigningKey) -> [u8; 64] {
        let payload = action.payload_for_signing();
        let signature = signing_key.sign(&payload);
        signature.to_bytes()
    }

    /// Helper to create a User storage action (Add) with proper signature
    fn create_signed_user_add_action(
        signing_key: &SigningKey,
        owner: PublicKey,
        id: Id,
        data: Vec<u8>,
        nonce: u64,
    ) -> Action {
        let timestamp = env::time_now();

        // Create metadata with placeholder signature
        let metadata = Metadata {
            created_at: timestamp,
            updated_at: timestamp.into(),
            storage_type: StorageType::User {
                owner,
                signature_data: Some(SignatureData {
                    signature: [0; 64], // Placeholder
                    nonce,
                }),
            },
        };

        // Create action for signing
        let mut action = Action::Add {
            id,
            data: data.clone(),
            ancestors: vec![],
            metadata: metadata.clone(),
        };

        // Sign and update action
        let signature = sign_action(&action, signing_key);

        if let Action::Add {
            ref mut metadata, ..
        } = action
        {
            if let StorageType::User {
                ref mut signature_data,
                ..
            } = metadata.storage_type
            {
                *signature_data = Some(SignatureData { signature, nonce });
            }
        }

        action
    }

    /// Helper to create a User storage Update action with proper signature
    fn create_signed_user_update_action(
        signing_key: &SigningKey,
        owner: PublicKey,
        id: Id,
        data: Vec<u8>,
        nonce: u64,
        created_at: u64,
    ) -> Action {
        let timestamp = env::time_now();

        let metadata = Metadata {
            created_at,
            updated_at: timestamp.into(),
            storage_type: StorageType::User {
                owner,
                signature_data: Some(SignatureData {
                    signature: [0; 64],
                    nonce,
                }),
            },
        };

        let mut action = Action::Update {
            id,
            data: data.clone(),
            ancestors: vec![],
            metadata: metadata.clone(),
        };

        let signature = sign_action(&action, signing_key);

        if let Action::Update {
            ref mut metadata, ..
        } = action
        {
            if let StorageType::User {
                ref mut signature_data,
                ..
            } = metadata.storage_type
            {
                *signature_data = Some(SignatureData { signature, nonce });
            }
        }

        action
    }

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
        assert!(MainInterface::apply_action(action).is_ok());

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
        let result = MainInterface::apply_action(action);
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
            },
        };

        // Missing signature should fail
        let result = MainInterface::apply_action(action);
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
        let result = MainInterface::apply_action(action);
        assert!(result.is_err());
        match result {
            Err(StorageError::InvalidSignature) => {}
            other => panic!("Expected InvalidSignature error, got {:?}", other),
        }
    }

    #[test]
    fn user_update_with_valid_signature_succeeds() {
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
        assert!(MainInterface::apply_action(action1).is_ok());

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

        assert!(MainInterface::apply_action(action2).is_ok());

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
    use calimero_primitives::identity::PublicKey;
    use ed25519_dalek::{Signer, SigningKey};

    use super::*;
    use crate::address::Id;
    use crate::env;

    fn create_test_keypair() -> (SigningKey, PublicKey) {
        let mut seed = [0u8; 32];
        env::random_bytes(&mut seed);
        let signing_key = SigningKey::from_bytes(&seed);
        let verifying_key = signing_key.verifying_key();
        let public_key = PublicKey::from(*verifying_key.as_bytes());
        (signing_key, public_key)
    }

    fn sign_action(action: &Action, signing_key: &SigningKey) -> [u8; 64] {
        let payload = action.payload_for_signing();
        let signature = signing_key.sign(&payload);
        signature.to_bytes()
    }

    fn create_signed_user_add_action(
        signing_key: &SigningKey,
        owner: PublicKey,
        id: Id,
        data: Vec<u8>,
        nonce: u64,
    ) -> Action {
        let timestamp = env::time_now();

        let metadata = Metadata {
            created_at: timestamp,
            updated_at: timestamp.into(),
            storage_type: StorageType::User {
                owner,
                signature_data: Some(SignatureData {
                    signature: [0; 64],
                    nonce,
                }),
            },
        };

        let mut action = Action::Add {
            id,
            data,
            ancestors: vec![],
            metadata,
        };

        let signature = sign_action(&action, signing_key);

        if let Action::Add {
            ref mut metadata, ..
        } = action
        {
            if let StorageType::User {
                ref mut signature_data,
                ..
            } = metadata.storage_type
            {
                *signature_data = Some(SignatureData { signature, nonce });
            }
        }

        action
    }

    fn create_signed_user_update_action(
        signing_key: &SigningKey,
        owner: PublicKey,
        id: Id,
        data: Vec<u8>,
        nonce: u64,
        created_at: u64,
    ) -> Action {
        let timestamp = env::time_now();

        let metadata = Metadata {
            created_at,
            updated_at: timestamp.into(),
            storage_type: StorageType::User {
                owner,
                signature_data: Some(SignatureData {
                    signature: [0; 64],
                    nonce,
                }),
            },
        };

        let mut action = Action::Update {
            id,
            data,
            ancestors: vec![],
            metadata,
        };

        let signature = sign_action(&action, signing_key);

        if let Action::Update {
            ref mut metadata, ..
        } = action
        {
            if let StorageType::User {
                ref mut signature_data,
                ..
            } = metadata.storage_type
            {
                *signature_data = Some(SignatureData { signature, nonce });
            }
        }

        action
    }

    #[test]
    fn replay_with_same_nonce_fails() {
        env::reset_for_testing();

        let (signing_key, owner) = create_test_keypair();

        let mut element = Element::root();
        element.set_user_domain(owner);
        let page = Page::new_from_element("Page", element);
        let serialized = to_vec(&page).unwrap();

        let nonce = env::time_now();

        // First action succeeds
        let action1 = create_signed_user_add_action(
            &signing_key,
            owner,
            page.id(),
            serialized.clone(),
            nonce,
        );
        assert!(MainInterface::apply_action(action1).is_ok());

        sleep(Duration::from_millis(2));

        // Second action with SAME nonce should fail (replay attack)
        let action2 = create_signed_user_update_action(
            &signing_key,
            owner,
            page.id(),
            serialized,
            nonce, // Same nonce!
            page.element().created_at(),
        );

        let result = MainInterface::apply_action(action2);
        assert!(result.is_err());
        match result {
            Err(StorageError::NonceReplay(_)) => {}
            other => panic!("Expected NonceReplay error, got {:?}", other),
        }
    }

    #[test]
    fn replay_with_lower_nonce_fails() {
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
        assert!(MainInterface::apply_action(action1).is_ok());

        sleep(Duration::from_millis(2));

        // Action with LOWER nonce should fail
        let nonce2 = nonce1 - 1000;
        let action2 = create_signed_user_update_action(
            &signing_key,
            owner,
            page.id(),
            serialized,
            nonce2, // Lower nonce!
            page.element().created_at(),
        );

        let result = MainInterface::apply_action(action2);
        assert!(result.is_err());
        match result {
            Err(StorageError::NonceReplay(_)) => {}
            other => panic!("Expected NonceReplay error, got {:?}", other),
        }
    }

    #[test]
    fn sequential_updates_with_increasing_nonces_succeed() {
        env::reset_for_testing();

        let (signing_key, owner) = create_test_keypair();

        let mut element = Element::root();
        element.set_user_domain(owner);
        let mut page = Page::new_from_element("Version 1", element);
        let serialized = to_vec(&page).unwrap();

        let nonce1 = env::time_now();
        let action1 =
            create_signed_user_add_action(&signing_key, owner, page.id(), serialized, nonce1);
        assert!(MainInterface::apply_action(action1).is_ok());

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
                MainInterface::apply_action(action).is_ok(),
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
    fn out_of_order_nonces_are_rejected() {
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
        assert!(MainInterface::apply_action(action_first).is_ok());

        sleep(Duration::from_millis(10));

        // Try action with nonce OLDER than stored updated_at - should fail
        // The replay protection compares nonce against stored updated_at
        let old_nonce = first_nonce - 1_000_000_000; // 1 second before first action
        let action_old = create_signed_user_update_action(
            &signing_key,
            owner,
            page.id(),
            serialized1,
            old_nonce,
            page.element().created_at(),
        );

        let result = MainInterface::apply_action(action_old);
        assert!(result.is_err());
        match result {
            Err(StorageError::NonceReplay(_)) => {}
            other => panic!("Expected NonceReplay error, got {:?}", other),
        }
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
            },
        };

        assert!(MainInterface::apply_action(action).is_ok());

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
            },
        };

        let result = MainInterface::apply_action(action);
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
            },
        };
        assert!(MainInterface::apply_action(add_action).is_ok());

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
            },
        };

        let result = MainInterface::apply_action(update_action);
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
            },
        };
        assert!(MainInterface::apply_action(add_action).is_ok());

        sleep(Duration::from_millis(2));

        // Try to delete - should fail
        let delete_action = Action::DeleteRef {
            id,
            deleted_at: env::time_now(),
            metadata: Metadata::default(),
        };

        let result = MainInterface::apply_action(delete_action);
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
            },
        };

        let result = MainInterface::apply_action(action);
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
            },
        };

        // This should succeed since the hash of empty [] matches
        assert!(MainInterface::apply_action(action).is_ok());
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
            },
        };

        let result = MainInterface::apply_action(action);
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
            },
        };

        // Should succeed since within tolerance
        assert!(MainInterface::apply_action(action).is_ok());
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
            },
        };

        // Past timestamps are fine
        assert!(MainInterface::apply_action(action).is_ok());
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

        let result = MainInterface::apply_action(action);
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
    use ed25519_dalek::{Signer, SigningKey};

    use super::*;
    use crate::address::Id;
    use crate::env;

    fn create_test_keypair() -> (SigningKey, PublicKey) {
        let mut seed = [0u8; 32];
        env::random_bytes(&mut seed);
        let signing_key = SigningKey::from_bytes(&seed);
        let verifying_key = signing_key.verifying_key();
        let public_key = PublicKey::from(*verifying_key.as_bytes());
        (signing_key, public_key)
    }

    fn sign_action(action: &Action, signing_key: &SigningKey) -> [u8; 64] {
        let payload = action.payload_for_signing();
        let signature = signing_key.sign(&payload);
        signature.to_bytes()
    }

    fn create_signed_user_add_action(
        signing_key: &SigningKey,
        owner: PublicKey,
        id: Id,
        data: Vec<u8>,
        nonce: u64,
    ) -> Action {
        let timestamp = env::time_now();

        let metadata = Metadata {
            created_at: timestamp,
            updated_at: timestamp.into(),
            storage_type: StorageType::User {
                owner,
                signature_data: Some(SignatureData {
                    signature: [0; 64],
                    nonce,
                }),
            },
        };

        let mut action = Action::Add {
            id,
            data,
            ancestors: vec![],
            metadata,
        };

        let signature = sign_action(&action, signing_key);

        if let Action::Add {
            ref mut metadata, ..
        } = action
        {
            if let StorageType::User {
                ref mut signature_data,
                ..
            } = metadata.storage_type
            {
                *signature_data = Some(SignatureData { signature, nonce });
            }
        }

        action
    }

    fn create_signed_user_update_action(
        signing_key: &SigningKey,
        owner: PublicKey,
        id: Id,
        data: Vec<u8>,
        nonce: u64,
        created_at: u64,
    ) -> Action {
        let timestamp = env::time_now();

        let metadata = Metadata {
            created_at,
            updated_at: timestamp.into(),
            storage_type: StorageType::User {
                owner,
                signature_data: Some(SignatureData {
                    signature: [0; 64],
                    nonce,
                }),
            },
        };

        let mut action = Action::Update {
            id,
            data,
            ancestors: vec![],
            metadata,
        };

        let signature = sign_action(&action, signing_key);

        if let Action::Update {
            ref mut metadata, ..
        } = action
        {
            if let StorageType::User {
                ref mut signature_data,
                ..
            } = metadata.storage_type
            {
                *signature_data = Some(SignatureData { signature, nonce });
            }
        }

        action
    }

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
                }),
            },
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
                *signature_data = Some(SignatureData { signature, nonce });
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
        assert!(MainInterface::apply_action(action1).is_ok());

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

        let result = MainInterface::apply_action(action2);
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
        assert!(MainInterface::apply_action(action1).is_ok());

        sleep(Duration::from_millis(2));

        // Delete with valid signature
        let nonce2 = env::time_now();
        let delete_action = create_signed_delete_action(&signing_key, owner, page.id(), nonce2);

        assert!(MainInterface::apply_action(delete_action).is_ok());

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
        assert!(MainInterface::apply_action(action1).is_ok());

        sleep(Duration::from_millis(2));

        // Try to delete with different owner's signature
        let nonce2 = env::time_now();
        let delete_action = create_signed_delete_action(&signing_key2, owner2, page.id(), nonce2);

        let result = MainInterface::apply_action(delete_action);
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
        assert!(MainInterface::apply_action(action1).is_ok());

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
            },
        };

        let result = MainInterface::apply_action(delete_action);
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

        let result = MainInterface::apply_action(action);
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
        assert!(MainInterface::apply_action(action1).is_ok());

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
            },
        };

        let result = MainInterface::apply_action(action2);
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
        assert!(MainInterface::apply_action(action1).is_ok());

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
        assert!(MainInterface::apply_action(action2).is_ok());

        sleep(Duration::from_millis(2));

        // Try to delete with old nonce (replay attack)
        let delete_action = create_signed_delete_action(&signing_key, owner, page.id(), nonce1);

        let result = MainInterface::apply_action(delete_action);
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
