//! The bytes one write ships in a delta, which every node stores in its DAG
//! history for as long as it keeps the delta. Row counters cannot see these:
//! a delta is one row, whatever it holds.
//!
//! ```text
//! cargo test -p storage-cost --test delta_bytes -- --nocapture
//! ```
//!
//! prints each action's breakdown.

use calimero_storage::action::Action;
use calimero_storage::collections::{Root, UnorderedMap, Vector};
use calimero_storage::delta::{clear_pending_delta, StorageDelta};
use calimero_storage::env::take_last_artifact;
use calimero_storage::store::MainStorage;
use storage_cost::measure;

type Map = UnorderedMap<String, String, MainStorage>;
type Nested = UnorderedMap<String, UnorderedMap<String, String, MainStorage>, MainStorage>;

/// One id: an entry's parent collection. The context root every chain ends at
/// is implied, and the count is a single byte.
const TOP_LEVEL_ENTRY_ANCESTOR_BYTES: usize = 1 + 32;

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

        for (label, value) in [("map insert", "value"), ("map overwrite", "value2")] {
            let actions = shipped(label, || {
                let mut map = Root::<Map>::fetch().expect("the root was just committed");
                map.insert("key_new".to_owned(), value.to_owned())
                    .expect("insert should succeed");
                map.commit();
            });
            assert_eq!(actions.len(), 1, "{label} ships the entry alone");
            assert_eq!(ancestor_bytes(&actions[0]), TOP_LEVEL_ENTRY_ANCESTOR_BYTES);
        }

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
