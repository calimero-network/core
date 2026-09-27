//! `Frozen<T>`: one value, written once, that no node lets anyone change.
//!
//! The value is guarded by its cell's writer set, whose one writer holds
//! `WRITE_ONCE` alone. These tests build a `Frozen<String>` through the API,
//! then play peers through `Interface::apply_action`: the creator trying to
//! rewrite or delete it, a stranger trying to write it, a fresh node taking it
//! at genesis, and sync redelivering it.

use ed25519_dalek::SigningKey;
use serial_test::serial;

use crate::address::Id;
use crate::collections::{Frozen, FrozenValue, Root};
use crate::entities::{ChildInfo, Data, Metadata, OpMask, StorageType};
use crate::env;
use crate::index::Index;
use crate::interface::{Interface, StorageError};
use crate::store::{Key, MainStorage, StorageAdaptor};
use crate::tests::common::{
    account_of_key, apply_ctx_for, build_signed_member_action, build_signed_member_delete,
};

type MainInterface = Interface<MainStorage>;

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

/// The founder's charter, as `init` would create it.
fn charter() -> (Root<Frozen<String>>, SigningKey) {
    env::reset_for_testing();
    let founder = key(0xF0);
    env::set_account_id(*account_of_key(&founder).as_bytes());
    let charter = Root::new(|| Frozen::new("be kind".to_owned()));
    (charter, founder)
}

fn value_bytes(id: Id, value: &str) -> Vec<u8> {
    borsh::to_vec(&(FrozenValue(value.to_owned()), id)).expect("serialize")
}

fn later() -> u64 {
    env::time_now() + 1_000_000_000
}

fn write(add: bool, ids: (Id, Id), value: &str, signer: &SigningKey) -> Result<(), StorageError> {
    let (anchor, value_id) = ids;
    let action = build_signed_member_action(
        add,
        value_id,
        anchor,
        value_bytes(value_id, value),
        later(),
        signer,
        vec![ChildInfo::new(anchor, [0; 32], Metadata::default())],
    );
    MainInterface::apply_action(action, &apply_ctx_for(account_of_key(signer)))
}

#[test]
#[serial]
fn a_frozen_value_reads_back_with_its_writer() {
    let (charter, founder) = charter();
    assert_eq!(charter.get().expect("get"), "be kind");
    assert_eq!(charter.writer(), Some(account_of_key(&founder)));
}

#[test]
#[serial]
fn its_writer_holds_write_once_and_nothing_else() {
    let (charter, founder) = charter();
    let (anchor, value_id) = charter.ids();
    let StorageType::Shared { writers, .. } = <Index<MainStorage>>::get_metadata(anchor)
        .expect("metadata")
        .expect("anchor")
        .storage_type
    else {
        panic!("the anchor is a writer-set cell");
    };
    assert_eq!(writers.len(), 1);
    let mask = writers[&account_of_key(&founder)];
    assert!(mask.contains(OpMask::WRITE_ONCE));
    for denied in [OpMask::WRITE, OpMask::DELETE, OpMask::ADMIN] {
        assert!(!mask.contains(denied), "{denied:?} must not be granted");
    }
    assert!(matches!(
        <Index<MainStorage>>::get_metadata(value_id)
            .expect("metadata")
            .expect("value")
            .storage_type,
        StorageType::SharedMember { anchor: a, .. } if a == anchor
    ));
}

#[test]
#[serial]
fn its_writer_cannot_rewrite_it_on_any_node() {
    let (charter, founder) = charter();
    assert!(matches!(
        write(false, charter.ids(), "be anything", &founder),
        Err(StorageError::ActionNotAllowed(_))
    ));
    assert!(matches!(
        write(true, charter.ids(), "be anything", &founder),
        Err(StorageError::ActionNotAllowed(_))
    ));
    assert_eq!(charter.get().expect("get"), "be kind");
}

#[test]
#[serial]
fn a_redelivery_of_the_same_value_is_accepted() {
    let (charter, founder) = charter();
    let (anchor, value_id) = charter.ids();
    let stored = MainStorage::storage_read(Key::Entry(value_id)).expect("stored");
    let action = build_signed_member_action(
        false,
        value_id,
        anchor,
        stored,
        later(),
        &founder,
        vec![ChildInfo::new(anchor, [0; 32], Metadata::default())],
    );
    MainInterface::apply_action(action, &apply_ctx_for(account_of_key(&founder)))
        .expect("sync redelivers what it already has");
}

#[test]
#[serial]
fn its_writer_cannot_delete_it_on_any_node() {
    let (charter, founder) = charter();
    let (anchor, value_id) = charter.ids();
    let removal = build_signed_member_delete(value_id, anchor, &founder, later());
    assert!(
        MainInterface::apply_action(removal, &apply_ctx_for(account_of_key(&founder))).is_err()
    );
    assert!(MainStorage::storage_read(Key::Entry(value_id)).is_some());
}

#[test]
#[serial]
fn nobody_else_can_write_it() {
    let (charter, _founder) = charter();
    let mallory = key(0xEE);
    assert!(write(false, charter.ids(), "be mean", &mallory).is_err());
    assert_eq!(charter.get().expect("get"), "be kind");
}

#[test]
#[serial]
fn a_fresh_node_takes_the_genesis_value_and_then_holds_it() {
    // A node that has the anchor but not yet the value, as a joiner mid-sync.
    let (charter, founder) = charter();
    let ids = charter.ids();
    let _ = MainStorage::storage_remove(Key::Entry(ids.1));
    write(true, ids, "be kind", &founder).expect("the genesis write lands");
    assert!(matches!(
        write(false, ids, "be anything", &founder),
        Err(StorageError::ActionNotAllowed(_))
    ));
}

#[test]
#[serial]
fn it_survives_the_post_init_reassignment() {
    let (mut charter, founder) = charter();
    charter.reassign_deterministic_id("charter");
    assert_eq!(charter.get().expect("get"), "be kind");
    assert_eq!(charter.writer(), Some(account_of_key(&founder)));
    let (anchor, _) = charter.ids();
    assert_eq!(anchor, charter.element().id());
    assert!(write(false, charter.ids(), "be anything", &founder).is_err());
}

#[test]
#[serial]
fn the_write_once_bit_is_a_defined_mask() {
    let bytes = borsh::to_vec(&OpMask::WRITE_ONCE).expect("serialize");
    let decoded: OpMask = borsh::from_slice(&bytes).expect("a defined bit decodes");
    assert_eq!(decoded, OpMask::WRITE_ONCE);
    assert!(
        borsh::from_slice::<OpMask>(&[0b1_0000]).is_err(),
        "an undefined bit does not"
    );
}
