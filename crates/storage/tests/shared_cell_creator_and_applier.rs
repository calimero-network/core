//! The node that creates a `SharedStorage` cell and a node that applies its deltas end with the
//! same anchor hash and the same context root hash, across the cell's whole life: creation, a
//! member entry, an update by a writer, a delete of the entry, a rotation (which writes nothing)
//! and a delete of the cell.
//!
//! The cell's wrapper carries the writers its id commits to and a rotation never rewrites it, so
//! a receiver that applies the creator's deltas cannot differ from the creator.
//!
//! Run with: `cargo test -p calimero-storage --features testing --test shared_cell_creator_and_applier`

#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;

use calimero_storage::address::Id;
use calimero_storage::collections::{LwwRegister, SharedStorage, UnorderedMap};
use calimero_storage::entities::Data;
use calimero_storage::env;
use calimero_storage::index::Index;
use calimero_storage::store::MainStorage;
use calimero_storage::testing::Script;
use serial_test::serial;

type Cell = SharedStorage<UnorderedMap<String, LwwRegister<String>>>;
type Cells = UnorderedMap<String, Cell>;

const NAME: &str = "cell";
const KEY: &str = "k";

/// The anchor's full hash and the context root hash, as the node reads them.
type Hashes = (Option<[u8; 32]>, Option<[u8; 32]>);

fn hashes(anchor: Id) -> Hashes {
    let anchor_hash = <Index<MainStorage>>::get_hashes_for(anchor)
        .unwrap()
        .map(|(full, _own)| full);
    (anchor_hash, env::root_hash())
}

/// The two replicas: one writes, the other applies what it wrote.
struct Pair {
    script: Script<Cells>,
    creator: usize,
    receiver: usize,
}

impl Pair {
    /// One step on the creator, then its delta, if it wrote one, on the receiver; the hashes the
    /// two then hold must agree.
    fn step(&mut self, label: &str, op: impl FnOnce(&mut Cells), anchor: Option<Id>) -> Hashes {
        let delta = self.script.run(self.creator, op);
        if let Some(delta) = delta {
            assert_eq!(
                self.script.deliver(self.receiver, delta),
                0,
                "{label}: nothing dropped"
            );
        }
        let read = |script: &Script<Cells>, replica| match anchor {
            Some(anchor) => script.view(replica, |_| hashes(anchor)),
            None => script.view(replica, |_| (None, env::root_hash())),
        };
        let made = read(&self.script, self.creator);
        assert_eq!(made, read(&self.script, self.receiver), "{label}: hashes");
        assert!(made.1.is_some(), "{label}: a root hash exists");
        made
    }
}

#[test]
#[serial]
fn a_node_that_applies_a_cells_deltas_ends_with_the_hashes_of_the_node_that_wrote_them() {
    let mut script = Script::new(Cells::new);
    let (creator, receiver) = (script.founder(), script.founder());
    let founder = script.account(creator);
    let mut pair = Pair {
        script,
        creator,
        receiver,
    };

    pair.step(
        "creation",
        |cells| {
            let _previous = cells
                .insert(NAME.to_owned(), Cell::new(BTreeSet::from([founder]), false))
                .unwrap();
        },
        None,
    );
    let anchor = pair
        .script
        .view(creator, |cells| {
            cells.get(NAME).unwrap().map(|cell| cell.element().id())
        })
        .unwrap();
    let (made, applied) = (
        pair.script.view(creator, |_| hashes(anchor)),
        pair.script.view(receiver, |_| hashes(anchor)),
    );
    assert_eq!(made, applied, "creation: anchor hash");
    assert!(made.0.is_some(), "the anchor exists");

    let write = |value: &'static str| {
        move |cells: &mut Cells| {
            let mut cell = cells.get_mut(NAME).unwrap().unwrap();
            let _previous = cell
                .get_mut()
                .unwrap()
                .insert(KEY.to_owned(), LwwRegister::new(value.to_owned()))
                .unwrap();
        }
    };
    pair.step("a member entry", write("v1"), Some(anchor));
    let before_rotation = pair.step("an update by a writer", write("v2"), Some(anchor));
    let rotated = pair.step(
        "a rotation",
        |cells| {
            let mut cell = cells.get_mut(NAME).unwrap().unwrap();
            let both = BTreeSet::from([founder, calimero_account::AccountId::from([0xEE; 32])]);
            cell.rotate_writers(both).unwrap();
        },
        Some(anchor),
    );
    assert_eq!(
        rotated.0, before_rotation.0,
        "a rotation writes nothing to the cell"
    );
    assert_eq!(
        env::take_recorded_rotations().len(),
        1,
        "the rotation was asked for"
    );
    pair.step(
        "a delete of the entry",
        |cells| {
            let mut cell = cells.get_mut(NAME).unwrap().unwrap();
            let _removed = cell.get_mut().unwrap().remove(KEY).unwrap();
        },
        Some(anchor),
    );
    pair.step(
        "a delete of the cell",
        |cells| {
            let _removed = cells.remove(NAME).unwrap();
        },
        Some(anchor),
    );
}
