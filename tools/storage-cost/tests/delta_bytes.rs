//! The bytes one write ships in a delta, which every node stores in its DAG
//! history for as long as it keeps the delta. Row counters cannot see these:
//! a delta is one row, whatever it holds.
//!
//! ```text
//! cargo test -p storage-cost --test delta_bytes -- --nocapture
//! ```
//!
//! prints each action's breakdown.

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::action::Action;
use calimero_storage::collections::{AuthoredVector, LwwRegister, Root, UnorderedMap, Vector};
use calimero_storage::delta::{clear_pending_delta, StorageDelta};
use calimero_storage::env::take_last_artifact;
use calimero_storage::store::MainStorage;
use storage_cost::measure;

type Map = UnorderedMap<String, String, MainStorage>;
type Nested = UnorderedMap<String, UnorderedMap<String, String, MainStorage>, MainStorage>;
/// kv-store's state.
type Kv = UnorderedMap<String, LwwRegister<String>, MainStorage>;

/// One id: an entry's parent collection. The context root every chain ends at
/// is implied, and the count is a single byte.
const TOP_LEVEL_ENTRY_ANCESTOR_BYTES: usize = 1 + 32;

/// An update to an entity every receiver already holds names no ancestor: the
/// receiver places it under the parent it stores. The count stays.
const UPDATE_ANCESTOR_BYTES: usize = 1;

/// Decode the artifact `write` commits, printing a line per action.
fn shipped(label: &str, write: impl FnOnce()) -> Vec<Action> {
    let _ignored = take_last_artifact();
    write();
    let artifact = take_last_artifact().expect("commit should emit a delta");
    let actions = match borsh::from_slice::<StorageDelta>(&artifact).expect("delta should decode") {
        StorageDelta::Actions(actions) | StorageDelta::CausalActions { actions, .. } => actions,
    };
    println!("{label}: {} B, {} actions", artifact.len(), actions.len());
    for action in &actions {
        let total = borsh::to_vec(action).expect("action encodes").len();
        match action {
            Action::Add { data, metadata, .. } | Action::Update { data, metadata, .. } => {
                println!(
                    "  {} {total} B = tag 1 + id 32 + data {} + ancestors {} + metadata {}",
                    if matches!(action, Action::Add { .. }) {
                        "add"
                    } else {
                        "update"
                    },
                    4 + data.len(),
                    ancestor_bytes(action),
                    borsh::to_vec(metadata).expect("metadata encodes").len(),
                );
            }
            Action::DeleteRef { .. } => println!("  delete {total} B"),
        }
    }
    actions
}

fn ancestor_bytes(action: &Action) -> usize {
    match action {
        Action::Add { ancestors, .. } | Action::Update { ancestors, .. } => {
            1 + 32 * ancestors.len()
        }
        Action::DeleteRef { .. } => 0,
    }
}

#[test]
fn an_entry_ships_its_parent_and_nothing_above_it() {
    measure(|| {
        clear_pending_delta();
        let mut map = Root::new(Map::new);
        for i in 0..100 {
            map.insert(format!("key{i}"), "value".to_owned())
                .expect("insert should succeed");
        }
        map.commit();

        let actions = shipped("map insert", || {
            let mut map = Root::<Map>::fetch().expect("the root was just committed");
            map.insert("key_new".to_owned(), "value".to_owned())
                .expect("insert should succeed");
            map.commit();
        });
        assert_eq!(actions.len(), 1, "an insert ships the entry alone");
        assert_eq!(ancestor_bytes(&actions[0]), TOP_LEVEL_ENTRY_ANCESTOR_BYTES);

        let mut vector = Root::new(Vector::<String, MainStorage>::new);
        vector
            .push("first".to_owned())
            .expect("push should succeed");
        vector.commit();
        let actions = shipped("vector push", || {
            let mut vector =
                Root::<Vector<String, MainStorage>>::fetch().expect("the root was just committed");
            vector
                .push("hello world message body".to_owned())
                .expect("push should succeed");
            vector.commit();
        });
        assert_eq!(ancestor_bytes(&actions[0]), TOP_LEVEL_ENTRY_ANCESTOR_BYTES);
    });
}

#[test]
fn only_a_direct_child_names_the_context_root() {
    measure(|| {
        clear_pending_delta();
        let actions = shipped("nested map create", || {
            let mut map = Root::new(Nested::new);
            let mut inner = UnorderedMap::new();
            inner
                .insert("a".to_owned(), "b".to_owned())
                .expect("insert should succeed");
            map.insert("outer".to_owned(), inner)
                .expect("insert should succeed");
            map.commit();
        });
        for action in &actions {
            if let Action::Add { ancestors, .. } | Action::Update { ancestors, .. } = action {
                let names_root = ancestors.iter().any(|ancestor| ancestor.id().is_root());
                assert!(
                    !names_root || ancestors.len() == 1,
                    "{:?} names the context root above its parent; every receiver \
                     already holds it",
                    action.id()
                );
            }
        }
    });
}

/// One chat message, shaped like mero-chat's (`tools/state-disk-cost`).
#[derive(BorshSerialize, BorshDeserialize)]
struct Message {
    sender: String,
    text: String,
    timestamp: u64,
    edited: bool,
    reply_to: Option<String>,
}

fn message(text: &str, edited: bool) -> Message {
    Message {
        sender: "alice".to_owned(),
        text: text.to_owned(),
        timestamp: 1_790_000_000_000,
        edited,
        reply_to: None,
    }
}

/// Only the parent's id ever told a receiver anything, and only for an entity
/// it might not hold. An update to one its writer held live before the write
/// is to one every receiver holds: an earlier action placed it, in this delta
/// or in one this delta follows.
#[test]
fn an_update_names_no_parent() {
    measure(|| {
        clear_pending_delta();
        let mut map = Root::new(Map::new);
        map.insert("key".to_owned(), "value".to_owned())
            .expect("insert should succeed");
        map.commit();
        let actions = shipped("map overwrite", || {
            let mut map = Root::<Map>::fetch().expect("the root was just committed");
            map.insert("key".to_owned(), "value2".to_owned())
                .expect("insert should succeed");
            map.commit();
        });
        assert!(matches!(actions[..], [Action::Update { .. }]));
        assert_eq!(ancestor_bytes(&actions[0]), UPDATE_ANCESTOR_BYTES);

        clear_pending_delta();
        let mut kv = Root::new(Kv::new);
        kv.insert("greeting".to_owned(), LwwRegister::new("hi".to_owned()))
            .expect("insert should succeed");
        kv.commit();
        let actions = shipped("kv-store update_if_exists", || {
            let mut kv = Root::<Kv>::fetch().expect("the root was just committed");
            kv.get_mut("greeting")
                .expect("get_mut should succeed")
                .expect("the key was just inserted")
                .set("hello".to_owned());
            kv.commit();
        });
        assert!(matches!(actions[..], [Action::Update { .. }]));
        assert_eq!(ancestor_bytes(&actions[0]), UPDATE_ANCESTOR_BYTES);

        clear_pending_delta();
        let mut chat = Root::new(AuthoredVector::<Message, MainStorage>::new);
        let id = chat
            .push(message("helo", false))
            .expect("push should succeed");
        chat.commit();
        let actions = shipped("chat message edit", || {
            let mut chat = Root::<AuthoredVector<Message, MainStorage>>::fetch()
                .expect("the root was just committed");
            chat.update_by_id(id, message("hello", true))
                .expect("update should succeed");
            chat.commit();
        });
        assert!(matches!(actions[..], [Action::Update { .. }]));
        assert_eq!(ancestor_bytes(&actions[0]), UPDATE_ANCESTOR_BYTES);

        clear_pending_delta();
        let mut nested = Root::new(Nested::new);
        let mut inner = UnorderedMap::new();
        inner
            .insert("a".to_owned(), "b".to_owned())
            .expect("insert should succeed");
        nested
            .insert("outer".to_owned(), inner)
            .expect("insert should succeed");
        nested.commit();
        let actions = shipped("nested map overwrite", || {
            let mut nested = Root::<Nested>::fetch().expect("the root was just committed");
            nested
                .get_mut("outer")
                .expect("get_mut should succeed")
                .expect("the key was just inserted")
                .insert("a".to_owned(), "c".to_owned())
                .expect("insert should succeed");
            nested.commit();
        });
        for action in &actions {
            if let Action::Update { .. } = action {
                assert_eq!(ancestor_bytes(action), UPDATE_ANCESTOR_BYTES);
            }
        }
    });
}

/// A receiver can have collected a tombstone its writer still holds, so an
/// entity written over one names its parent, as a new one does.
#[test]
fn a_rewrite_of_a_deleted_entry_names_its_parent() {
    measure(|| {
        clear_pending_delta();
        let mut map = Root::new(Map::new);
        map.insert("key".to_owned(), "value".to_owned())
            .expect("insert should succeed");
        map.commit();
        let mut map = Root::<Map>::fetch().expect("the root was just committed");
        let _removed = map.remove("key").expect("remove should succeed");
        map.commit();

        let actions = shipped("map re-insert", || {
            let mut map = Root::<Map>::fetch().expect("the root was just committed");
            map.insert("key".to_owned(), "again".to_owned())
                .expect("insert should succeed");
            map.commit();
        });
        for action in &actions {
            if let Action::Add { .. } | Action::Update { .. } = action {
                assert_eq!(ancestor_bytes(action), TOP_LEVEL_ENTRY_ANCESTOR_BYTES);
            }
        }
    });
}
