//! Integration tests for sync operations.

use calimero_storage::address::{Id, Path};
use calimero_storage::collections::unordered::Bag;
use calimero_storage::entities::{Data, Element};
use calimero_storage::env::MockedStorage;
use calimero_storage::index::Index;
use calimero_storage::Interface;

use crate::{full, SyncState};

// Test entity types (same as in storage/src/tests/common.rs)
#[derive(Clone, Debug, PartialEq, Eq, borsh::BorshSerialize, borsh::BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct Page {
    element: Element,
    title: String,
    paragraphs: Bag<Paragraph>,
}

impl Data for Page {
    fn element(&self) -> &Element {
        &self.element
    }

    fn element_mut(&mut self) -> &mut Element {
        &mut self.element
    }
}

impl Page {
    fn new_from_element(title: &str, element: Element) -> Self {
        Page {
            element,
            title: title.to_string(),
            paragraphs: Bag::new(),
        }
    }

    fn id(&self) -> Id {
        self.element.id()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, borsh::BorshSerialize, borsh::BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct Paragraph {
    element: Element,
    text: String,
}

impl Data for Paragraph {
    fn element(&self) -> &Element {
        &self.element
    }

    fn element_mut(&mut self) -> &mut Element {
        &mut self.element
    }
}

impl Paragraph {
    fn new_from_element(text: &str, element: Element) -> Self {
        Paragraph {
            element,
            text: text.to_string(),
        }
    }

    fn id(&self) -> Id {
        self.element.id()
    }
}

type TestStorage = MockedStorage<1000>;
type TestInterface = Interface<TestStorage>;

#[cfg(test)]
mod snapshot_tests {
    use super::*;

    #[test]
    fn generate_snapshot() {
        use calimero_storage::address::Path;

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
        let snapshot = full::generate_snapshot::<TestStorage>().unwrap();

        // Verify snapshot contains data
        assert!(snapshot.entity_count > 0);
        assert!(snapshot.index_count > 0);
        assert_ne!(snapshot.root_hash, [0; 32]);
        assert!(snapshot.timestamp > 0);

        // Verify specific entities are included
        let entry_ids: Vec<Id> = snapshot.entries.iter().map(|(id, _)| *id).collect();
        assert!(entry_ids.contains(&page.id()));
        assert!(entry_ids.contains(&para1.id()));
        assert!(entry_ids.contains(&para2.id()));
    }

    #[test]
    fn apply_snapshot() {
        use calimero_storage::address::Path;
        type ForeignStorage = MockedStorage<99>;
        type ForeignInterface = Interface<ForeignStorage>;

        // Create data on foreign node - page as root, para as child
        let mut foreign_page = Page::new_from_element("Foreign Page", Element::root());
        let para_path = Path::new("::foreign_para").unwrap();
        let mut foreign_para = Paragraph::new_from_element(
            "Foreign Para",
            Element::new(&para_path, None),
        );

        ForeignInterface::save(&mut foreign_page).unwrap();
        ForeignInterface::add_child_to(
            foreign_page.id(),
            &foreign_page.paragraphs,
            &mut foreign_para,
        )
        .unwrap();

        // Generate snapshot from foreign
        let snapshot = ForeignInterface::generate_snapshot().unwrap();

        // Apply snapshot to TestInterface (which is empty)
        assert!(full::apply_snapshot::<TestStorage>(&snapshot).is_ok());

        // Verify data was restored
        let retrieved_page = TestInterface::find_by_id::<Page>(foreign_page.id()).unwrap();
        assert!(retrieved_page.is_some());
        assert_eq!(retrieved_page.unwrap().title, "Foreign Page");

        let retrieved_para = TestInterface::find_by_id::<Paragraph>(foreign_para.id()).unwrap();
        assert!(retrieved_para.is_some());
        assert_eq!(retrieved_para.unwrap().text, "Foreign Para");
    }

    #[test]
    fn snapshot_excludes_tombstones() {
        use calimero_storage::index::Index;
        use calimero_storage::address::Path;

        // Create parent page as root
        let mut page = Page::new_from_element("Parent Page", Element::root());

        // Create paragraphs with unique paths
        let para1_path = Path::new("::para1").unwrap();
        let para2_path = Path::new("::para2").unwrap();
        let mut para1 = Paragraph::new_from_element(
            "Para 1",
            Element::new(&para1_path, None),
        );
        let mut para2 = Paragraph::new_from_element(
            "Para 2",
            Element::new(&para2_path, None),
        );

        TestInterface::save(&mut page).unwrap();
        TestInterface::add_child_to(page.id(), &page.paragraphs, &mut para1).unwrap();
        TestInterface::add_child_to(page.id(), &page.paragraphs, &mut para2).unwrap();

        // Delete para2 (creates tombstone)
        TestInterface::remove_child_from(page.id(), &page.paragraphs, para2.id()).unwrap();

        // Verify para2 is tombstoned
        assert!(<Index<TestStorage>>::is_deleted(para2.id()).unwrap());

        // Generate snapshot
        let snapshot = full::generate_snapshot::<TestStorage>().unwrap();

        // Verify para1 is in snapshot, but para2 (tombstone) is not
        let entry_ids: Vec<Id> = snapshot.entries.iter().map(|(id, _)| *id).collect();
        assert!(entry_ids.contains(&para1.id()), "Para1 should be in snapshot");
        assert!(!entry_ids.contains(&para2.id()), "Para2 (deleted) should NOT be in snapshot");
    }

    #[test]
    fn full_resync_complete_flow() {
        use calimero_storage::address::Path;
        type ForeignStorage = MockedStorage<99>;
        type ForeignInterface = Interface<ForeignStorage>;

        // Create local data - page as root with a local paragraph
        let mut local_page = Page::new_from_element("Local Page", Element::root());
        let local_para_path = Path::new("::local_para").unwrap();
        let mut local_para = Paragraph::new_from_element(
            "Local Para",
            Element::new(&local_para_path, None),
        );

        TestInterface::save(&mut local_page).unwrap();
        TestInterface::add_child_to(local_page.id(), &local_page.paragraphs, &mut local_para).unwrap();
        let local_para_id = local_para.id();

        // Create foreign data - page as root with a foreign paragraph
        let mut foreign_page = Page::new_from_element("Foreign Page", Element::root());
        let foreign_para_path = Path::new("::foreign_para").unwrap();
        let mut foreign_para = Paragraph::new_from_element(
            "Foreign Para",
            Element::new(&foreign_para_path, None),
        );

        ForeignInterface::save(&mut foreign_page).unwrap();
        ForeignInterface::add_child_to(
            foreign_page.id(),
            &foreign_page.paragraphs,
            &mut foreign_para,
        )
        .unwrap();

        // Generate snapshot from foreign
        let snapshot = ForeignInterface::generate_snapshot().unwrap();

        // Perform full resync
        let remote_node_id = Id::random();
        assert!(full::full_resync::<TestStorage>(remote_node_id, snapshot).is_ok());

        // Verify local data was replaced with foreign data
        // Note: Root page has same ID on both, so title will be from foreign
        let retrieved_page = TestInterface::find_by_id::<Page>(foreign_page.id()).unwrap();
        assert!(retrieved_page.is_some());
        assert_eq!(retrieved_page.unwrap().title, "Foreign Page");

        // Verify foreign paragraph exists
        let retrieved_foreign_para = TestInterface::find_by_id::<Paragraph>(foreign_para.id()).unwrap();
        assert!(retrieved_foreign_para.is_some());

        // Verify old local paragraph is gone (different ID from foreign)
        let old_local_para = TestInterface::find_by_id::<Paragraph>(local_para_id).unwrap();
        assert!(old_local_para.is_none());

        // Verify sync state was updated
        let sync_state = SyncState::get_sync_state::<TestStorage>(remote_node_id).unwrap();
        assert!(sync_state.is_some());
        let state = sync_state.unwrap();
        assert_eq!(state.sync_count, 1);
        assert_eq!(state.node_id, remote_node_id);
    }
}

#[cfg(test)]
mod sync_state_tests {
    use super::*;

    #[test]
    fn needs_full_resync_fresh_node() {
        let remote_id = Id::random();
        let retention_ns = 86400_000_000_000u64; // 1 day

        // Fresh node (no sync state) should need full resync
        assert!(SyncState::needs_full_resync::<TestStorage>(remote_id, retention_ns).unwrap());
    }

    #[test]
    fn needs_full_resync_recent_sync() {
        use calimero_storage::env::time_now;

        let remote_id = Id::random();
        let retention_ns = 86400_000_000_000u64; // 1 day

        // Create recent sync state
        let state = SyncState {
            node_id: remote_id,
            last_sync_nanos: time_now(),
            sync_count: 1,
        };

        SyncState::save_sync_state::<TestStorage>(&state).unwrap();

        // Recent sync should NOT need full resync
        assert!(!SyncState::needs_full_resync::<TestStorage>(remote_id, retention_ns).unwrap());
    }

    #[test]
    fn needs_full_resync_old_sync() {
        use calimero_storage::env::time_now;

        let remote_id = Id::random();
        let retention_ns = 1_000_000_000u64; // 1 second

        // Create old sync state (older than retention)
        let state = SyncState {
            node_id: remote_id,
            last_sync_nanos: time_now() - (retention_ns * 2), // 2x retention ago
            sync_count: 1,
        };

        SyncState::save_sync_state::<TestStorage>(&state).unwrap();

        // Old sync should need full resync
        assert!(SyncState::needs_full_resync::<TestStorage>(remote_id, retention_ns).unwrap());
    }
}

