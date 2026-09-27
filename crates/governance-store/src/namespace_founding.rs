//! What a namespace this node founded was derived from: its founder and salt.
//!
//! A namespace root created without a caller-chosen id gets
//! `calimero_account::founded_namespace_id(founder, salt)` as its id (see
//! `calimero_account`'s `namespace_id` module for why). The salt is the half
//! nobody can recompute, so the founding node keeps the pair and hands it to
//! whoever asks it to show that its account founded the namespace.
//!
//! # Why the founder is stored, not re-read
//!
//! The founder is the account the node held *when it founded*. Re-deriving it
//! from the node's current account root would give the wrong answer after an
//! account import, and a pair that does not reproduce the id proves nothing.
//! So the pair is stored together, and [`NamespaceFoundingRepository::record`]
//! refuses one that does not reproduce the id: a stored row always verifies.
//!
//! # Why a local row, and only local
//!
//! The pair proves something about the id, not about governance, so it has no
//! place in folded state: no apply reads it, no peer needs it to converge, and
//! a namespace founded before derivation existed simply has none. It is written
//! once, by the node that minted it, and never changed.
//!
//! It is not a secret either. Disclosing it lets anyone confirm who founded the
//! namespace, which is the point, and grants nothing: the id commits to the
//! founder, so the salt cannot be replayed for another account.
//!
//! # Losing it
//!
//! A founding node that loses its store loses the salt, and with it the only
//! copy. The namespace keeps working — nothing in core depends on the row — but
//! the founder can no longer demonstrate founding to a third party. Carrying
//! the salt on the genesis op, which is the replica-verifiable half of #2932,
//! removes that single copy; it is a wire change and lands separately.

use calimero_account::{is_founded_by, AccountId, NAMESPACE_SALT_LEN};
use calimero_context_config::types::ContextGroupId;
use calimero_store::key::Generic as GenericKey;
use calimero_store::slice::Slice;
use calimero_store::types::GenericData;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

/// 16-byte `Generic` scope for founding records, keyed by namespace id.
/// Distinct from every other `calimero-*` scope in the tree.
const FOUNDING_SCOPE: [u8; 16] = *b"calimero-nsfound";

/// A stored record: the founder's account id, then the salt.
const RECORD_LEN: usize = 32 + NAMESPACE_SALT_LEN;

fn key(namespace_id: &ContextGroupId) -> GenericKey {
    GenericKey::new(FOUNDING_SCOPE, namespace_id.to_bytes())
}

/// Read/write access to the founding records of namespaces this node founded.
pub struct NamespaceFoundingRepository<'a> {
    store: &'a Store,
}

impl<'a> NamespaceFoundingRepository<'a> {
    #[must_use]
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    /// Record that `founder` founded `namespace_id` with `salt`.
    ///
    /// Refused unless the pair reproduces the id — a record that does not is a
    /// caller bug and would prove nothing. Idempotent for the same pair; a
    /// different pair for the same id is refused rather than overwritten,
    /// since exactly one pair reproduces a derived id and replacing it would
    /// destroy the only copy of the right one.
    ///
    /// # Errors
    /// A pair that does not reproduce the id, a conflicting record, or the
    /// store read/write.
    pub fn record(
        &self,
        namespace_id: &ContextGroupId,
        founder: &AccountId,
        salt: &[u8; NAMESPACE_SALT_LEN],
    ) -> EyreResult<()> {
        if !is_founded_by(&namespace_id.to_bytes(), founder, salt) {
            bail!(
                "refusing to record a founding for namespace {namespace_id:?}: account \
                 {founder} with this salt does not derive that id"
            );
        }
        if let Some(existing) = self.get(namespace_id)? {
            if existing == (*founder, *salt) {
                return Ok(());
            }
            bail!(
                "refusing to replace the founding recorded for namespace {namespace_id:?}: \
                 a derived id has exactly one founder and salt"
            );
        }
        let mut bytes = Vec::with_capacity(RECORD_LEN);
        bytes.extend_from_slice(founder.as_bytes());
        bytes.extend_from_slice(salt);
        let data = GenericData::from(Slice::from(bytes));
        self.store.handle().put(&key(namespace_id), &data)?;
        Ok(())
    }

    /// The founder and salt `namespace_id` was derived from, if this node
    /// founded it with one.
    ///
    /// `None` for a namespace this node did not found, and for one founded
    /// before derivation existed.
    ///
    /// # Errors
    /// The store read, or a stored row of the wrong length.
    pub fn get(
        &self,
        namespace_id: &ContextGroupId,
    ) -> EyreResult<Option<(AccountId, [u8; NAMESPACE_SALT_LEN])>> {
        let handle = self.store.handle();
        let Some(data) = handle.get(&key(namespace_id))? else {
            return Ok(None);
        };
        let bytes: &[u8] = data.as_ref();
        if bytes.len() != RECORD_LEN {
            bail!(
                "founding record for namespace {namespace_id:?} is {} bytes, expected {RECORD_LEN}",
                bytes.len()
            );
        }
        let (founder, salt) = bytes.split_at(32);
        let mut founder_bytes = [0u8; 32];
        founder_bytes.copy_from_slice(founder);
        let mut salt_bytes = [0u8; NAMESPACE_SALT_LEN];
        salt_bytes.copy_from_slice(salt);
        Ok(Some((AccountId::from(founder_bytes), salt_bytes)))
    }
}

#[cfg(test)]
mod tests {
    use calimero_account::founded_namespace_id;

    use super::*;
    use crate::test_fixtures::test_store;

    const SALT: [u8; 32] = [0x22; 32];

    fn founder() -> AccountId {
        AccountId::from([0x11; 32])
    }

    fn derived() -> ContextGroupId {
        ContextGroupId::from(founded_namespace_id(&founder(), &SALT))
    }

    #[test]
    fn a_recorded_founding_reads_back() {
        let store = test_store();
        let repo = NamespaceFoundingRepository::new(&store);
        assert_eq!(repo.get(&derived()).unwrap(), None);
        repo.record(&derived(), &founder(), &SALT).unwrap();
        assert_eq!(repo.get(&derived()).unwrap(), Some((founder(), SALT)));
    }

    #[test]
    fn recording_the_same_pair_again_is_a_no_op() {
        let store = test_store();
        let repo = NamespaceFoundingRepository::new(&store);
        repo.record(&derived(), &founder(), &SALT).unwrap();
        repo.record(&derived(), &founder(), &SALT).unwrap();
        assert_eq!(repo.get(&derived()).unwrap(), Some((founder(), SALT)));
    }

    #[test]
    fn a_pair_that_does_not_derive_the_id_is_refused() {
        let store = test_store();
        let repo = NamespaceFoundingRepository::new(&store);
        let random = ContextGroupId::from([0x5a; 32]);
        assert!(repo.record(&random, &founder(), &SALT).is_err());
        assert!(repo
            .record(&derived(), &AccountId::from([0x12; 32]), &SALT)
            .is_err());
        assert_eq!(repo.get(&random).unwrap(), None);
        assert_eq!(repo.get(&derived()).unwrap(), None);
    }

    #[test]
    fn records_are_per_namespace() {
        let store = test_store();
        let repo = NamespaceFoundingRepository::new(&store);
        repo.record(&derived(), &founder(), &SALT).unwrap();
        let other = ContextGroupId::from(founded_namespace_id(&founder(), &[0x23; 32]));
        assert_eq!(repo.get(&other).unwrap(), None);
    }
}
