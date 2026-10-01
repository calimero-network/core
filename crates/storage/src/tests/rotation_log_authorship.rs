//! Who may change a cell's writer set: entries planted in its rotation log,
//! and snapshot leaves that call themselves rotation-log book-keeping.

use std::collections::BTreeSet;

use borsh::to_vec;
use serial_test::serial;

use crate::address::Id;
use crate::collections::crdt_meta::CrdtType;
use crate::collections::{LwwRegister, Root, WriterSetCell};
use crate::entities::{Metadata, StorageType};
use crate::env;
use crate::index::Index;
use crate::interface::Interface;
use crate::store::{Key, MainStorage, StorageAdaptor};
use crate::tests::common::{account_of_key, writers_of};
use crate::tests::owned_rules::{act_as, key, text};

type MainInterface = Interface<MainStorage>;
type Cell = WriterSetCell<LwwRegister<String>>;

const ALICE: u8 = 0xA1;
const MALLORY: u8 = 0xEE;

/// Alice's cell, with her as its one writer: its anchor and value ids.
fn alices_cell() -> (Id, Id) {
    let alice = key(ALICE);
    let writers: BTreeSet<_> = [act_as(&alice)].into_iter().collect();
    let mut cell = Root::new(|| Cell::new_with_field_name("doc", writers, false));
    let _ = cell.insert(text("alice's value")).expect("insert");
    (cell.anchor(), cell.value_id())
}

/// A leaf's rotation-log label does not waive its signature check, for a writer
/// set or for a value.
#[test]
#[serial]
fn a_snapshot_leaf_labelled_rotation_log_is_still_verified() {
    env::reset_for_testing();
    let (anchor, value) = alices_cell();
    let mallory = key(MALLORY);
    let stored = |id| -> Metadata {
        <Index<MainStorage>>::get_metadata(id)
            .expect("metadata")
            .expect("entity")
    };
    let parent = |id| <Index<MainStorage>>::get_parent_id(id).expect("parent");

    let mut writer_set = stored(anchor);
    writer_set.storage_type = StorageType::Shared {
        writers: writers_of([account_of_key(&mallory)]),
        signature_data: None,
    };
    let anchor_bytes = MainStorage::storage_read(Key::Entry(anchor)).expect("anchor");
    let mut member = stored(value);
    if let StorageType::SharedMember { signature_data, .. } = &mut member.storage_type {
        *signature_data = None;
    }
    let value_bytes = to_vec(&(text("mallory's value"), value)).expect("serialize");

    let verdicts = |writer_set: &Metadata, member: &Metadata| {
        (
            MainInterface::verify_snapshot_entity_signature(
                anchor,
                parent(anchor),
                &anchor_bytes,
                writer_set,
            ),
            MainInterface::verify_snapshot_entity_signature(
                value,
                parent(value),
                &value_bytes,
                member,
            ),
        )
    };
    let (anchor_verdict, value_verdict) = verdicts(&writer_set, &member);
    assert!(anchor_verdict.is_err() && value_verdict.is_err(), "control");

    writer_set.crdt_type = Some(CrdtType::RotationLog);
    member.crdt_type = Some(CrdtType::RotationLog);
    let (anchor_verdict, value_verdict) = verdicts(&writer_set, &member);
    assert!(
        anchor_verdict.is_err(),
        "a relabelled, unsigned writer set must be refused"
    );
    assert!(
        value_verdict.is_err(),
        "a relabelled, unsigned value must be refused"
    );
}
