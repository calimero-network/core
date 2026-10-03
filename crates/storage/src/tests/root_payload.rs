//! A peer's delta can carry any bytes at the app root's fixed ids, the root
//! collection (`Id::root()`) and its single entry (`ROOT_ENTRY_ID`). Every
//! node loads both on its next call through `Root::fetch` and `Root::get`,
//! which panic on a read that fails, so one delta stored there would stop
//! every node that applied it. `Root::sync` reads both back before it returns
//! and refuses the delta instead; the node stores none of a refused sync's
//! writes, which this mocked store does not model.

use serial_test::serial;

use crate::action::Action;
use crate::address::Id;
use crate::collections::{Counter, Root, ROOT_ENTRY_ID};
use crate::delta::StorageDelta;
use crate::entities::Metadata;
use crate::env;
use crate::interface::{ApplyContext, Interface, StorageError};
use crate::merge::register_crdt_merge;
use crate::store::MainStorage;

type S = MainStorage;

/// Not a borsh `Collection` nor a `Counter`: the first byte is not a valid
/// `Option` tag, as in the fuzz input that found this.
const GARBAGE: [u8; 8] = [9; 8];

fn seeded_root() {
    env::reset_for_testing();
    register_crdt_merge::<Counter>();
    let mut root = Root::<Counter, S>::new(Counter::new);
    root.increment().unwrap();
    root.commit();
}

fn peer_add(id: Id) -> Action {
    Action::Add {
        id,
        data: GARBAGE.to_vec(),
        ancestors: vec![],
        metadata: Metadata::new(u64::MAX / 2, u64::MAX / 2),
    }
}

fn sync(actions: Vec<Action>) -> Result<(), StorageError> {
    let payload = borsh::to_vec(&StorageDelta::Actions(actions)).unwrap();
    Root::<Counter, S>::sync(&payload, &ApplyContext::empty())
}

#[test]
#[serial]
fn a_root_payload_that_is_not_the_root_type_is_refused() {
    seeded_root();

    let refused = sync(vec![peer_add(Id::root())]);

    assert!(
        matches!(refused, Err(StorageError::DeserializationError(_))),
        "{refused:?}"
    );
}

/// However an unreadable entry came to be stored, a sync does not commit over
/// it: `Root::get` would panic on the next call.
#[test]
#[serial]
fn a_sync_over_a_root_entry_that_does_not_decode_is_refused() {
    seeded_root();
    let _ = Interface::<S>::save_raw(ROOT_ENTRY_ID, GARBAGE.to_vec(), Metadata::default()).unwrap();

    let refused = sync(vec![]);

    assert!(
        matches!(refused, Err(StorageError::DeserializationError(_))),
        "{refused:?}"
    );
}
