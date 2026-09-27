//! `Frozen<T>`: one value, written once, that no node lets anyone change.
//!
//! The value is guarded by its cell's writer set, whose one writer holds
//! `WRITE_ONCE` alone. These tests build a `Frozen<String>` through the API,
//! then play peers through `Interface::apply_action`: the creator trying to
//! rewrite or delete it, a stranger trying to write it, a fresh node taking it
//! at genesis, and sync redelivering it.

use borsh::{BorshDeserialize, BorshSerialize};
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

/// A content-addressed value written on one node reads back on a node that
/// received it by delta: the `frozen-storage` and `frozen-rga-convergence`
/// scenarios on real nodes.
#[test]
#[serial]
fn a_content_addressed_value_reads_back_on_a_peer() {
    use crate::action::Action;
    use crate::collections::{FrozenStorage, Mergeable};
    use crate::delta::{commit_causal_delta, reset_delta_context, set_current_heads, StorageDelta};
    use crate::interface::ApplyContext;
    use crate::merge::register_crdt_merge;

    #[derive(borsh::BorshSerialize, borsh::BorshDeserialize)]
    struct Doc {
        items: FrozenStorage<String>,
    }
    impl crate::collections::rekey::RekeyTarget for Doc {
        fn rekey_relative_to(&mut self, parent_id: Id) {
            crate::rekey_field_if_supported!(
                &mut self.items,
                crate::collections::rekey::field_child_id(parent_id, "items")
            );
        }
    }
    #[diagnostic::do_not_recommend]
    impl crate::collections::crdt_meta::MergeStrategy for Doc {
        const DISPATCHED: bool = false;
    }
    impl Mergeable for Doc {
        fn merge(&mut self, other: &Self) -> Result<(), crate::collections::crdt_meta::MergeError> {
            self.items.merge(&other.items)
        }
    }

    let root_hash = || {
        <Index<MainStorage>>::get_hashes_for(Id::root())
            .unwrap()
            .map(|(full, _)| full)
            .unwrap_or([0; 32])
    };
    let capture = |root_data: Vec<u8>| -> Vec<Action> {
        MainInterface::save_raw(Id::root(), root_data, Metadata::default()).unwrap();
        commit_causal_delta(&root_hash())
            .unwrap()
            .expect("a delta")
            .actions
    };
    let import = |actions: Vec<Action>| {
        let payload = borsh::to_vec(&StorageDelta::Actions(actions)).unwrap();
        Root::<Doc, MainStorage>::sync(&payload, &ApplyContext::empty()).unwrap();
    };
    let fresh_node = |device: [u8; 32]| {
        env::reset_for_testing();
        reset_delta_context();
        register_crdt_merge::<Doc>();
        set_current_heads(vec![[0; 32]]);
        env::set_device_id(device);
    };

    fresh_node([9; 32]);
    let genesis = Root::<Doc, MainStorage>::new(|| Doc {
        items: FrozenStorage::new(),
    });
    let data = borsh::to_vec(&*genesis).unwrap();
    drop(genesis);
    let base = capture(data);
    let base_hash = root_hash();

    fresh_node([2; 32]);
    import(base.clone());
    reset_delta_context();
    set_current_heads(vec![base_hash]);
    let mut writer = Root::<Doc, MainStorage>::fetch().unwrap();
    let hash = writer.items.insert("immutable".to_owned()).unwrap();
    assert_eq!(
        writer.items.get(&hash).unwrap().as_deref(),
        Some("immutable")
    );
    // What a host-side repair infers the `Frozen` stamp from: a frozen entry
    // carries no wire authorization. Untagged, it lands as `Public`, and the
    // collection's read filter hides it on the repaired node.
    let entry = crate::collections::compute_id(writer.items.inner_id(), &hash);
    assert_eq!(
        <Index<MainStorage>>::get_metadata(entry)
            .unwrap()
            .expect("entry")
            .crdt_type,
        Some(crate::collections::crdt_meta::CrdtType::FrozenStorage)
    );
    let data = borsh::to_vec(&*writer).unwrap();
    drop(writer);
    let insert = capture(data);
    let writer_hash = root_hash();

    fresh_node([1; 32]);
    import(base);
    reset_delta_context();
    set_current_heads(vec![base_hash]);
    import(insert);
    assert_eq!(root_hash(), writer_hash, "the peer converges");
    let reader = Root::<Doc, MainStorage>::fetch().unwrap();
    assert_eq!(
        reader.items.get(&hash).unwrap().as_deref(),
        Some("immutable"),
        "the peer reads the value"
    );
}

/// A founding record, as an app would freeze one.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, Default, PartialEq)]
struct Charter {
    name: String,
    founded_at: u64,
    members: Vec<String>,
}

/// `Frozen<T>` holds for every value type, not only `String`: the value reads
/// back, its writer is its creator, and no node accepts a rewrite or a removal.
macro_rules! frozen_value_tests {
    ($name:ident, $ty:ty, $value:expr, $other:expr) => {
        mod $name {
            use super::*;

            fn frozen() -> (Root<Frozen<$ty>>, SigningKey) {
                env::reset_for_testing();
                let founder = key(0xF0);
                env::set_account_id(*account_of_key(&founder).as_bytes());
                (Root::new(|| Frozen::new($value)), founder)
            }

            fn put(ids: (Id, Id), value: $ty, signer: &SigningKey) -> Result<(), StorageError> {
                let (anchor, value_id) = ids;
                let bytes = borsh::to_vec(&(FrozenValue(value), value_id)).expect("serialize");
                let action = build_signed_member_action(
                    false,
                    value_id,
                    anchor,
                    bytes,
                    later(),
                    signer,
                    vec![ChildInfo::new(anchor, [0; 32], Metadata::default())],
                );
                MainInterface::apply_action(action, &apply_ctx_for(account_of_key(signer)))
            }

            #[test]
            #[serial]
            fn it_reads_back_with_its_creator_as_writer() {
                let (value, founder) = frozen();
                assert_eq!(*value.get().expect("get"), $value);
                assert_eq!(value.writer(), Some(account_of_key(&founder)));
            }

            #[test]
            #[serial]
            fn no_node_accepts_a_rewrite_even_by_its_writer() {
                let (value, founder) = frozen();
                assert!(matches!(
                    put(value.ids(), $other, &founder),
                    Err(StorageError::ActionNotAllowed(_))
                ));
                assert!(put(value.ids(), $other, &key(0xEE)).is_err());
                assert_eq!(*value.get().expect("get"), $value);
            }

            #[test]
            #[serial]
            fn a_redelivery_of_the_same_value_is_accepted() {
                let (value, founder) = frozen();
                put(value.ids(), $value, &founder).expect("sync redelivers it");
                assert_eq!(*value.get().expect("get"), $value);
            }

            #[test]
            #[serial]
            fn no_node_accepts_a_removal() {
                let (value, founder) = frozen();
                let (anchor, value_id) = value.ids();
                let removal = build_signed_member_delete(value_id, anchor, &founder, later());
                assert!(MainInterface::apply_action(
                    removal,
                    &apply_ctx_for(account_of_key(&founder))
                )
                .is_err());
                assert_eq!(*value.get().expect("get"), $value);
            }

            #[test]
            #[serial]
            fn it_survives_the_post_init_reassignment() {
                let (mut value, founder) = frozen();
                value.reassign_deterministic_id("field");
                assert_eq!(*value.get().expect("get"), $value);
                assert!(put(value.ids(), $other, &founder).is_err());
            }
        }
    };
}

frozen_value_tests!(of_u64, u64, 1_700_000_000, 1);
frozen_value_tests!(of_bool, bool, true, false);
frozen_value_tests!(
    of_a_list,
    Vec<String>,
    vec!["alice".to_owned(), "bob".to_owned()],
    vec!["mallory".to_owned()]
);
frozen_value_tests!(of_an_option, Option<String>, Some("v1".to_owned()), None);
frozen_value_tests!(
    of_a_struct,
    Charter,
    Charter {
        name: "calimero".to_owned(),
        founded_at: 7,
        members: vec!["alice".to_owned()],
    },
    Charter::default()
);
