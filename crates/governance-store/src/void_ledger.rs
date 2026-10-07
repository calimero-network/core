//! What a node remembers to take back its own judgement of a void op (the ops it judged
//! void, the key each applied rotation introduced), and what became of each parked op.

use std::collections::BTreeSet;
use std::sync::Mutex;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_governance_types::NamespaceId;
use calimero_store::iter::DBIter;
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

const UNDECIDED: u8 = 0; // a parked op's row before a replay judges it at its cut
const REFUSED: u8 = 1; // a parked op's row once a replay refused it at its cut
const DELETE_BATCH: usize = 1000; // parked rows removed per store pass on leaving

/// What became of an op kept unread on arrival, until a replay applies it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parked {
    /// No replay has judged it yet: it folds as a hole.
    Undecided,
    /// A replay refused it at its cut: it folds as nothing.
    Refused,
}

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

/// The parked row a raw `Generic` key names, if it lies under `scope`.
fn parked_row(scope: [u8; 16], key: &[u8]) -> Option<GenericKey> {
    let fragment: [u8; 32] = key.strip_prefix(&scope[..])?.try_into().ok()?;
    Some(GenericKey::new(scope, fragment))
}

/// What became of `op`, which this node parked in `namespace`; `None` when it holds no
/// mark (never parked, or since applied).
pub fn parked_op(
    store: &Store,
    namespace: NamespaceId,
    op: [u8; 32],
) -> EyreResult<Option<Parked>> {
    VoidLedger::new(store, namespace).parked(op)
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

    /// Remember that `op` was kept unread on arrival, so a replay judges it.
    pub(crate) fn note_parked(&self, op: [u8; 32]) -> EyreResult<()> {
        self.put_parked(op, UNDECIDED)
    }

    /// Record a replay's verdict on `op` at its cut; `false` when it was not parked,
    /// since a replay may refuse what an arrival applied.
    pub(crate) fn settle_parked(&self, op: [u8; 32], applied: bool) -> EyreResult<bool> {
        let _guard = WRITE_LOCK.lock().unwrap_or_else(|held| held.into_inner());
        if self.parked(op)?.is_none() {
            return Ok(false);
        }
        if applied {
            self.forget_parked(op)?;
        } else {
            self.put_parked(op, REFUSED)?;
        }
        Ok(true)
    }

    pub(crate) fn parked(&self, op: [u8; 32]) -> EyreResult<Option<Parked>> {
        let handle = self.store.handle();
        let Some(data) = handle.get(&self.parked_key(op))? else {
            return Ok(None);
        };
        let bytes: &[u8] = data.as_ref();
        Ok(Some(if bytes == [REFUSED] {
            Parked::Refused
        } else {
            Parked::Undecided
        }))
    }

    #[cfg(test)]
    pub(crate) fn refused(&self, op: [u8; 32]) -> EyreResult<bool> {
        Ok(self.parked(op)? == Some(Parked::Refused))
    }

    pub(crate) fn forget_parked(&self, op: [u8; 32]) -> EyreResult<()> {
        self.store.handle().delete(&self.parked_key(op))?;
        Ok(())
    }

    /// Forget everything kept for the namespace, as when this node leaves it.
    pub(crate) fn clear(&self) -> EyreResult<()> {
        let _guard = WRITE_LOCK.lock().unwrap_or_else(|held| held.into_inner());
        let mut handle = self.store.handle();
        for kind in [&b"voided"[..], b"keys", b"defaults"] {
            handle.delete(&self.key(kind))?;
        }
        drop(handle);
        let scope = self.parked_scope();
        loop {
            let rows = self.parked_rows(scope)?;
            if rows.is_empty() {
                return Ok(());
            }
            let mut handle = self.store.handle();
            for row in rows {
                handle.delete(&row)?;
            }
        }
    }

    /// Up to [`DELETE_BATCH`] parked rows under `scope`. Read as raw keys, since the
    /// typed iterator needs a key error type `Generic` does not have.
    fn parked_rows(&self, scope: [u8; 16]) -> EyreResult<Vec<GenericKey>> {
        let handle = self.store.handle();
        let mut iter = handle.iter::<GenericKey>()?;
        let mut start = [0; 48];
        start[..16].copy_from_slice(&scope);
        let mut rows = Vec::new();
        let mut next = DBIter::seek(&mut iter, Slice::from(&start[..]))?
            .and_then(|key| parked_row(scope, key.as_ref()));
        while let Some(row) = next {
            rows.push(row);
            if rows.len() == DELETE_BATCH {
                break;
            }
            next = DBIter::next(&mut iter)?.and_then(|key| parked_row(scope, key.as_ref()));
        }
        Ok(rows)
    }

    fn key(&self, kind: &[u8]) -> GenericKey {
        let mut hasher = Sha256::new();
        hasher.update(kind);
        hasher.update(self.namespace.as_bytes());
        GenericKey::new(SCOPE, hasher.finalize().into())
    }

    /// Each namespace's parked rows share one scope, so leaving it can find them all.
    fn parked_scope(&self) -> [u8; 16] {
        let mut hasher = Sha256::new();
        hasher.update(b"calimero-parked");
        hasher.update(self.namespace.as_bytes());
        let digest: [u8; 32] = hasher.finalize().into();
        let mut scope = [0; 16];
        scope.copy_from_slice(&digest[..16]);
        scope
    }

    fn parked_key(&self, op: [u8; 32]) -> GenericKey {
        GenericKey::new(self.parked_scope(), op)
    }

    fn put_parked(&self, op: [u8; 32], verdict: u8) -> EyreResult<()> {
        self.store.handle().put(
            &self.parked_key(op),
            &GenericData::from(Slice::from(vec![verdict])),
        )?;
        Ok(())
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
    fn a_parked_op_is_refused_until_a_replay_applies_it() {
        let store = test_store();
        let ledger = VoidLedger::new(&store, NamespaceId::from([1u8; 32]));
        ledger.note_parked([7; 32]).unwrap();
        assert_eq!(ledger.parked([7; 32]).unwrap(), Some(Parked::Undecided));

        assert!(ledger.settle_parked([7; 32], false).unwrap());
        assert_eq!(ledger.parked([7; 32]).unwrap(), Some(Parked::Refused));
        assert!(ledger.settle_parked([7; 32], true).unwrap());
        assert_eq!(
            ledger.parked([7; 32]).unwrap(),
            None,
            "a later apply takes it back"
        );
        assert!(!ledger.settle_parked([7; 32], false).unwrap());
        assert_eq!(
            ledger.parked([7; 32]).unwrap(),
            None,
            "a replay of an applied op does not judge it again"
        );
    }

    #[test]
    fn a_replay_refuses_nothing_that_was_not_parked() {
        let store = test_store();
        let ledger = VoidLedger::new(&store, NamespaceId::from([1u8; 32]));
        assert!(!ledger.settle_parked([7; 32], false).unwrap());
        assert_eq!(ledger.parked([7; 32]).unwrap(), None);
    }

    #[test]
    fn leaving_forgets_parked_rows_of_that_namespace_only() {
        let store = test_store();
        let left = VoidLedger::new(&store, NamespaceId::from([1u8; 32]));
        let kept = VoidLedger::new(&store, NamespaceId::from([2u8; 32]));
        left.note_parked([7; 32]).unwrap();
        let _ = left.settle_parked([7; 32], false).unwrap();
        left.note_parked([8; 32]).unwrap();
        kept.note_parked([7; 32]).unwrap();

        left.clear().unwrap();

        assert_eq!(left.parked([7; 32]).unwrap(), None);
        assert_eq!(left.parked([8; 32]).unwrap(), None);
        assert_eq!(kept.parked([7; 32]).unwrap(), Some(Parked::Undecided));
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
