//! Which founder, and which exact genesis op, founded a replica's copy of a
//! namespace created before ids were derived.
//!
//! A namespace founded before derivation has a random id and a plain
//! `RootOp::NamespaceCreated { founder, .. }` genesis. Nothing reproduces its
//! id, so [`NamespaceFoundingRepository`](crate::NamespaceFoundingRepository)
//! has no row for it. This one records what such a genesis does carry: the
//! founder it names, and the genesis op's content hash — the id the op has in
//! the namespace governance DAG (`SignedNamespaceOp::content_hash`, the
//! `delta_id` the op log is keyed by). A client holding the op can check the
//! hash; a client comparing replicas can see whether they were founded by the
//! same op.
//!
//! # Deliberately separate from the derived-id record
//!
//! A pair in `NamespaceFoundingRepository` proves founding: the id commits to
//! the founder. This row proves nothing of the kind — a forged plain genesis on
//! a bare replica would be recorded just the same, which is exactly the gap
//! derived ids close. Keeping it under its own scope and its own API field
//! (`legacyFounding`, never inside `founding`) is what stops a legacy founder
//! from ever satisfying a derived-id check.
//!
//! # Who writes it
//!
//! The plain-genesis apply, on every replica, in the same two places the
//! derived-id row is recorded: the establish branch, and the founding node's
//! own genesis-shaped re-arrival. It is hash-neutral like the deny-list — a
//! local `Generic` row that records a fact the genesis carries, read by no
//! apply. It is never derived from mutable meta (`owner_identity`,
//! `admin_identity`): the founder comes from the genesis op itself.
//!
//! # Backfill
//!
//! Nodes that applied their plain geneses before this row existed have none.
//! [`NamespaceLegacyFoundingRepository::get_or_backfill`] fills it lazily on
//! read from the genesis op in the local op log: the one stored root op that is
//! a plain `NamespaceCreated`, parentless, validly signed, and signed by the
//! device the founder's credential certifies. If the log holds none, or holds
//! more than one such op (a later parentless genesis is stored as a no-op on an
//! established namespace, and nothing local says which one founded it without
//! reading mutable meta), it answers absent and writes nothing. A namespace
//! with a derived-id record is never backfilled.

use calimero_account::AccountId;
use calimero_context_client::local_governance::{NamespaceOp, RootOp};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_types::NamespaceId;
use calimero_store::key::Generic as GenericKey;
use calimero_store::slice::Slice;
use calimero_store::types::GenericData;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use crate::ops::namespace::member_joined_open::join_op_proves_ownership;
use crate::{NamespaceFoundingRepository, NamespaceOpLogService};

/// 16-byte `Generic` scope for legacy founding records, keyed by namespace id.
/// Distinct from every other `calimero-*` scope in the tree, the derived-id
/// `calimero-nsfound` included.
const LEGACY_FOUNDING_SCOPE: [u8; 16] = *b"calimero-nslegcy";

/// A stored record: the founder's account id, then the genesis op hash.
const RECORD_LEN: usize = 32 + 32;

fn key(namespace_id: &ContextGroupId) -> GenericKey {
    GenericKey::new(LEGACY_FOUNDING_SCOPE, namespace_id.to_bytes())
}

/// Read/write access to the legacy (plain-genesis) founding records.
pub struct NamespaceLegacyFoundingRepository<'a> {
    store: &'a Store,
}

impl<'a> NamespaceLegacyFoundingRepository<'a> {
    #[must_use]
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    /// Record that `founder` founded this replica's copy of `namespace_id`
    /// with the genesis op whose content hash is `genesis_op_hash`.
    ///
    /// Idempotent for the same pair. A different pair for the same id is
    /// refused rather than overwritten: a replica's copy is founded by one
    /// genesis, and the first one recorded is the one that founded it.
    ///
    /// # Errors
    /// A conflicting record, or the store read/write.
    pub fn record(
        &self,
        namespace_id: &ContextGroupId,
        founder: &AccountId,
        genesis_op_hash: &[u8; 32],
    ) -> EyreResult<()> {
        if let Some(existing) = self.get(namespace_id)? {
            if existing == (*founder, *genesis_op_hash) {
                return Ok(());
            }
            bail!(
                "refusing to replace the legacy founding recorded for namespace \
                 {namespace_id:?}: this copy was founded by genesis {}",
                hex::encode(existing.1)
            );
        }
        let mut bytes = Vec::with_capacity(RECORD_LEN);
        bytes.extend_from_slice(founder.as_bytes());
        bytes.extend_from_slice(genesis_op_hash);
        let data = GenericData::from(Slice::from(bytes));
        self.store.handle().put(&key(namespace_id), &data)?;
        Ok(())
    }

    /// The founder and genesis op hash recorded for `namespace_id`, if any.
    ///
    /// Reads the row only; see [`Self::get_or_backfill`] for nodes that
    /// applied their genesis before the row existed.
    ///
    /// # Errors
    /// The store read, or a stored row of the wrong length.
    pub fn get(&self, namespace_id: &ContextGroupId) -> EyreResult<Option<(AccountId, [u8; 32])>> {
        let handle = self.store.handle();
        let Some(data) = handle.get(&key(namespace_id))? else {
            return Ok(None);
        };
        let bytes: &[u8] = data.as_ref();
        if bytes.len() != RECORD_LEN {
            bail!(
                "legacy founding record for namespace {namespace_id:?} is {} bytes, \
                 expected {RECORD_LEN}",
                bytes.len()
            );
        }
        let (founder, hash) = bytes.split_at(32);
        let mut founder_bytes = [0u8; 32];
        founder_bytes.copy_from_slice(founder);
        let mut hash_bytes = [0u8; 32];
        hash_bytes.copy_from_slice(hash);
        Ok(Some((AccountId::from(founder_bytes), hash_bytes)))
    }

    /// [`Self::get`], filling the row from the stored genesis op when it is
    /// missing — the lazy backfill for nodes that applied their plain genesis
    /// before this row existed (see the module docs for which op qualifies).
    ///
    /// `None` for a namespace with a derived-id record, for one whose op log
    /// holds no qualifying genesis, and for one whose log holds more than one.
    ///
    /// # Errors
    /// The store reads, or the backfill write.
    pub fn get_or_backfill(
        &self,
        namespace_id: &ContextGroupId,
    ) -> EyreResult<Option<(AccountId, [u8; 32])>> {
        if let Some(found) = self.get(namespace_id)? {
            return Ok(Some(found));
        }
        if NamespaceFoundingRepository::new(self.store)
            .get(namespace_id)?
            .is_some()
        {
            return Ok(None);
        }
        let Some((founder, hash)) = self.stored_plain_genesis(namespace_id)? else {
            return Ok(None);
        };
        self.record(namespace_id, &founder, &hash)?;
        Ok(Some((founder, hash)))
    }

    /// The single stored plain genesis for `namespace_id`, if exactly one.
    fn stored_plain_genesis(
        &self,
        namespace_id: &ContextGroupId,
    ) -> EyreResult<Option<(AccountId, [u8; 32])>> {
        let ns = NamespaceId::from(namespace_id.to_bytes());
        let mut found: Option<(AccountId, [u8; 32])> = None;
        for op in NamespaceOpLogService::new(self.store, ns).collect_root_ops()? {
            let NamespaceOp::Root(RootOp::NamespaceCreated { founder, account }) = &op.op else {
                continue;
            };
            // The same shape the establish branch demands of a founding op. A
            // stored op was verified when it applied; re-checking keeps the
            // backfill from trusting the log alone.
            if !op.parent_op_hashes.is_empty()
                || op.verify_signature().is_err()
                || !join_op_proves_ownership(&op.signer, founder, account)
            {
                continue;
            }
            let hash = op
                .content_hash()
                .map_err(|e| eyre::eyre!("content_hash: {e}"))?;
            match found {
                None => found = Some((*founder, hash)),
                Some((_, seen)) if seen == hash => {}
                Some(_) => {
                    tracing::warn!(
                        namespace_id = %hex::encode(namespace_id.to_bytes()),
                        "more than one stored plain genesis; not backfilling the legacy \
                         founding record"
                    );
                    return Ok(None);
                }
            }
        }
        Ok(found)
    }

    /// Drop the record, with the rest of the namespace's local state.
    ///
    /// # Errors
    /// The store write.
    pub fn delete(&self, namespace_id: &ContextGroupId) -> EyreResult<()> {
        self.store.handle().delete(&key(namespace_id))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::test_store;

    const HASH: [u8; 32] = [0x22; 32];

    fn founder() -> AccountId {
        AccountId::from([0x11; 32])
    }

    fn namespace() -> ContextGroupId {
        ContextGroupId::from([0x5a; 32])
    }

    #[test]
    fn a_recorded_legacy_founding_reads_back() {
        let store = test_store();
        let repo = NamespaceLegacyFoundingRepository::new(&store);
        assert_eq!(repo.get(&namespace()).unwrap(), None);
        repo.record(&namespace(), &founder(), &HASH).unwrap();
        assert_eq!(repo.get(&namespace()).unwrap(), Some((founder(), HASH)));
    }

    #[test]
    fn recording_the_same_pair_again_is_a_no_op() {
        let store = test_store();
        let repo = NamespaceLegacyFoundingRepository::new(&store);
        repo.record(&namespace(), &founder(), &HASH).unwrap();
        repo.record(&namespace(), &founder(), &HASH).unwrap();
        assert_eq!(repo.get(&namespace()).unwrap(), Some((founder(), HASH)));
    }

    #[test]
    fn a_different_pair_for_the_same_namespace_is_refused() {
        let store = test_store();
        let repo = NamespaceLegacyFoundingRepository::new(&store);
        repo.record(&namespace(), &founder(), &HASH).unwrap();
        assert!(repo.record(&namespace(), &founder(), &[0x23; 32]).is_err());
        assert!(repo
            .record(&namespace(), &AccountId::from([0x12; 32]), &HASH)
            .is_err());
        assert_eq!(repo.get(&namespace()).unwrap(), Some((founder(), HASH)));
    }

    #[test]
    fn records_are_per_namespace() {
        let store = test_store();
        let repo = NamespaceLegacyFoundingRepository::new(&store);
        repo.record(&namespace(), &founder(), &HASH).unwrap();
        let other = ContextGroupId::from([0x5b; 32]);
        assert_eq!(repo.get(&other).unwrap(), None);
        repo.delete(&namespace()).unwrap();
        assert_eq!(repo.get(&namespace()).unwrap(), None);
    }

    #[test]
    fn the_derived_id_record_is_a_different_row() {
        let store = test_store();
        NamespaceLegacyFoundingRepository::new(&store)
            .record(&namespace(), &founder(), &HASH)
            .unwrap();
        assert_eq!(
            NamespaceFoundingRepository::new(&store)
                .get(&namespace())
                .unwrap(),
            None
        );
    }

    #[test]
    fn backfill_with_no_stored_genesis_is_absent() {
        let store = test_store();
        let repo = NamespaceLegacyFoundingRepository::new(&store);
        assert_eq!(repo.get_or_backfill(&namespace()).unwrap(), None);
        assert_eq!(repo.get(&namespace()).unwrap(), None);
    }
}
