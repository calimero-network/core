//! What a node remembers to take back its own judgement of a void op: the ops it
//! judged void, and the key each applied rotation introduced. Also which parked ops
//! their retried apply refused, which the projection folds as nothing.

use std::collections::BTreeSet;
use std::sync::Mutex;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_governance_types::NamespaceId;
use calimero_store::key::Generic as GenericKey;
use calimero_store::slice::Slice;
use calimero_store::types::GenericData;
use calimero_store::Store;
use eyre::Result as EyreResult;
use sha2::{Digest, Sha256};

/// 16-byte `Generic` scope of the rows.
const SCOPE: [u8; 16] = *b"calimero-voidldg";

/// Serializes the read-modify-write of a row.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Stands in for the op of a key that was already held when an op stored it: no void
/// verdict on that op takes the key back.
pub(crate) const STORED_BEFORE: [u8; 32] = [0; 32];

const PARKED: u8 = 0; // a parked op's row: no retried apply has decided it yet
const REFUSED: u8 = 1; // a parked op's row: its last retried apply refused it

/// The default capabilities a group held before the first op this node applied set them
/// (`None` for none), which a rollback restores when no other op sets one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, BorshSerialize, BorshDeserialize)]
pub(crate) struct DefaultSeed {
    pub(crate) group: [u8; 32],
    pub(crate) caps: Option<u32>,
}

/// A key an applied op stored: the op, the group, and the key's id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, BorshSerialize, BorshDeserialize)]
pub(crate) struct KeyIntro {
    pub(crate) group: [u8; 32],
    pub(crate) op: [u8; 32],
    pub(crate) key: [u8; 32],
}

pub(crate) struct VoidLedger<'a> {
    store: &'a Store,
    namespace: NamespaceId,
}

impl<'a> VoidLedger<'a> {
    pub(crate) fn new(store: &'a Store, namespace: NamespaceId) -> Self {
        Self { store, namespace }
    }

    /// The ops last judged void, which a later removal may put back.
    pub(crate) fn voided(&self) -> EyreResult<BTreeSet<[u8; 32]>> {
        self.read(b"voided")
    }

    pub(crate) fn set_voided(&self, ids: &BTreeSet<[u8; 32]>) -> EyreResult<()> {
        let _guard = WRITE_LOCK.lock().unwrap_or_else(|held| held.into_inner());
        self.write(b"voided", ids)
    }

    pub(crate) fn note_voided(&self, id: [u8; 32]) -> EyreResult<()> {
        let _guard = WRITE_LOCK.lock().unwrap_or_else(|held| held.into_inner());
        let mut ids = self.read::<[u8; 32]>(b"voided")?;
        if ids.insert(id) {
            self.write(b"voided", &ids)?;
        }
        Ok(())
    }

    /// Every key an applied op stored. Only a key listed here is ever marked void.
    pub(crate) fn key_intros(&self) -> EyreResult<BTreeSet<KeyIntro>> {
        self.read(b"keys")
    }

    pub(crate) fn note_key_intro(&self, intro: KeyIntro) -> EyreResult<()> {
        let _guard = WRITE_LOCK.lock().unwrap_or_else(|held| held.into_inner());
        let mut intros = self.read::<KeyIntro>(b"keys")?;
        if intros.insert(intro) {
            self.write(b"keys", &intros)?;
        }
        Ok(())
    }

    /// Remember what `group`'s default capabilities were before the first op this node
    /// applied set them. Kept once.
    pub(crate) fn note_default_seed(&self, group: [u8; 32], caps: Option<u32>) -> EyreResult<()> {
        let _guard = WRITE_LOCK.lock().unwrap_or_else(|held| held.into_inner());
        let mut seeds = self.read::<DefaultSeed>(b"defaults")?;
        if seeds.iter().any(|seed| seed.group == group) {
            return Ok(());
        }
        let _ = seeds.insert(DefaultSeed { group, caps });
        self.write(b"defaults", &seeds)
    }

    /// What `group`'s default capabilities were before an op set them, if one did.
    pub(crate) fn default_seed(&self, group: [u8; 32]) -> EyreResult<Option<Option<u32>>> {
        Ok(self
            .read::<DefaultSeed>(b"defaults")?
            .into_iter()
            .find(|seed| seed.group == group)
            .map(|seed| seed.caps))
    }

    /// Remember that `op` was kept unread on arrival, so its retried apply decides it.
    pub(crate) fn note_parked(&self, op: [u8; 32]) -> EyreResult<()> {
        self.store.handle().put(
            &self.op_key(op),
            &GenericData::from(Slice::from(vec![PARKED])),
        )?;
        Ok(())
    }

    /// Record how the retried apply of `op` went, if it was parked. An op applied on
    /// arrival is not judged again by a replay, which may refuse what it once applied.
    pub(crate) fn settle_parked(&self, op: [u8; 32], applied: bool) -> EyreResult<()> {
        let _guard = WRITE_LOCK.lock().unwrap_or_else(|held| held.into_inner());
        let key = self.op_key(op);
        let mut handle = self.store.handle();
        if handle.get(&key)?.is_none() {
            return Ok(());
        }
        if applied {
            handle.delete(&key)?;
        } else {
            handle.put(&key, &GenericData::from(Slice::from(vec![REFUSED])))?;
        }
        Ok(())
    }

    /// Whether the last retried apply of the parked op `op` refused it.
    pub(crate) fn refused(&self, op: [u8; 32]) -> EyreResult<bool> {
        let handle = self.store.handle();
        let Some(data) = handle.get(&self.op_key(op))? else {
            return Ok(false);
        };
        let bytes: &[u8] = data.as_ref();
        Ok(bytes == [REFUSED])
    }

    pub(crate) fn forget_parked(&self, op: [u8; 32]) -> EyreResult<()> {
        self.store.handle().delete(&self.op_key(op))?;
        Ok(())
    }

    /// Forget everything kept for the namespace, as when this node leaves it.
    pub(crate) fn clear(&self) -> EyreResult<()> {
        let _guard = WRITE_LOCK.lock().unwrap_or_else(|held| held.into_inner());
        let mut handle = self.store.handle();
        for kind in [&b"voided"[..], b"keys", b"defaults"] {
            handle.delete(&self.key(kind))?;
        }
        Ok(())
    }

    fn key(&self, kind: &[u8]) -> GenericKey {
        let mut hasher = Sha256::new();
        hasher.update(kind);
        hasher.update(self.namespace.as_bytes());
        GenericKey::new(SCOPE, hasher.finalize().into())
    }

    fn op_key(&self, op: [u8; 32]) -> GenericKey {
        let mut hasher = Sha256::new();
        hasher.update(b"parked");
        hasher.update(self.namespace.as_bytes());
        hasher.update(op);
        GenericKey::new(SCOPE, hasher.finalize().into())
    }

    fn read<T: BorshDeserialize + Ord>(&self, kind: &[u8]) -> EyreResult<BTreeSet<T>> {
        let handle = self.store.handle();
        let Some(data) = handle.get(&self.key(kind))? else {
            return Ok(BTreeSet::new());
        };
        let bytes: &[u8] = data.as_ref();
        Ok(borsh::from_slice(bytes)?)
    }

    fn write<T: BorshSerialize>(&self, kind: &[u8], rows: &BTreeSet<T>) -> EyreResult<()> {
        let data = GenericData::from(Slice::from(borsh::to_vec(rows)?));
        self.store.handle().put(&self.key(kind), &data)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::test_store;

    #[test]
    fn what_is_noted_is_read_back_once() {
        let store = test_store();
        let ledger = VoidLedger::new(&store, NamespaceId::from([1u8; 32]));
        assert!(ledger.voided().unwrap().is_empty());

        ledger.note_voided([7; 32]).unwrap();
        ledger.note_voided([7; 32]).unwrap();
        ledger.note_voided([3; 32]).unwrap();
        assert_eq!(ledger.voided().unwrap().len(), 2);

        let intro = KeyIntro {
            group: [1; 32],
            op: [2; 32],
            key: [3; 32],
        };
        ledger.note_key_intro(intro).unwrap();
        ledger.note_key_intro(intro).unwrap();
        assert_eq!(ledger.key_intros().unwrap(), BTreeSet::from([intro]));

        ledger.set_voided(&BTreeSet::from([[9; 32]])).unwrap();
        assert_eq!(ledger.voided().unwrap(), BTreeSet::from([[9; 32]]));
    }

    #[test]
    fn a_default_seed_is_kept_from_the_first_note_only() {
        let store = test_store();
        let ledger = VoidLedger::new(&store, NamespaceId::from([1u8; 32]));
        assert_eq!(ledger.default_seed([4; 32]).unwrap(), None, "none recorded");

        ledger.note_default_seed([4; 32], None).unwrap();
        ledger.note_default_seed([4; 32], Some(8)).unwrap();
        assert_eq!(ledger.default_seed([4; 32]).unwrap(), Some(None));
        ledger.note_default_seed([5; 32], Some(8)).unwrap();
        assert_eq!(ledger.default_seed([5; 32]).unwrap(), Some(Some(8)));
    }

    #[test]
    fn a_parked_op_is_refused_until_a_retry_applies_it() {
        let store = test_store();
        let ledger = VoidLedger::new(&store, NamespaceId::from([1u8; 32]));
        ledger.note_parked([7; 32]).unwrap();
        assert!(
            !ledger.refused([7; 32]).unwrap(),
            "undecided is not refused"
        );

        ledger.settle_parked([7; 32], false).unwrap();
        assert!(ledger.refused([7; 32]).unwrap());
        ledger.settle_parked([7; 32], true).unwrap();
        assert!(
            !ledger.refused([7; 32]).unwrap(),
            "a later apply takes it back"
        );
        ledger.settle_parked([7; 32], false).unwrap();
        assert!(
            !ledger.refused([7; 32]).unwrap(),
            "a replay of an applied op does not judge it again"
        );
    }

    #[test]
    fn a_replay_refuses_nothing_that_was_not_parked() {
        let store = test_store();
        let ledger = VoidLedger::new(&store, NamespaceId::from([1u8; 32]));
        ledger.settle_parked([7; 32], false).unwrap();
        assert!(!ledger.refused([7; 32]).unwrap());
    }

    #[test]
    fn a_namespace_keeps_its_own_rows() {
        let store = test_store();
        VoidLedger::new(&store, NamespaceId::from([1u8; 32]))
            .note_voided([7; 32])
            .unwrap();
        assert!(VoidLedger::new(&store, NamespaceId::from([2u8; 32]))
            .voided()
            .unwrap()
            .is_empty());
    }
}
