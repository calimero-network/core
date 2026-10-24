//! What a derived namespace id was derived from: its founder and salt.
//!
//! Every namespace root gets `calimero_account::founded_namespace_id(founder,
//! salt)` as its id, except an account namespace (`RootOp::AccountNamespaceCreated`),
//! which gets `account_namespace_id(founder, salt)`; its genesis carries the salt (see
//! `calimero_account`'s `namespace_id` module for why). This row is where a node
//! keeps that pair so it can show it: anyone holding both can confirm which
//! account founded the namespace without holding any of its governance state.
//!
//! # Who writes it
//!
//! The genesis apply, on every replica — the founding node included, which
//! applies its own genesis like any peer. So every member and replica of a
//! derived namespace holds the same row, and showing founding does not depend
//! on the founder's node still existing.
//!
//! # Why a local row, not folded state
//!
//! It records a fact the genesis already carries, so it is hash-neutral like
//! the deny-list: every replica that applies the same genesis derives the same
//! row, so an apply may read it (`GroupCreated` does, to spot a namespace root).
//!
//! # Why the founder is stored with the salt
//!
//! A pair that does not reproduce the id proves nothing, so
//! [`NamespaceFoundingRepository::record`] refuses one — a stored row always
//! verifies. Storing the founder rather than re-deriving it also keeps the row
//! right on the founding node after an account import changes its current
//! account.
//!
//! Disclosing the pair grants nothing: the id commits to the founder, so the
//! salt cannot be replayed for another account. The account namespace's salt is
//! derived from the account root's secret so its id cannot be found from the
//! account id; its genesis, and so this row, are held only by that namespace's
//! own members.

use calimero_account::{is_account_namespace_of, is_founded_by, AccountId, NAMESPACE_SALT_LEN};
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

/// Which genesis founded a namespace, and so which derivation its id must match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenesisKind {
    Ordinary,
    AccountNamespace,
}

impl GenesisKind {
    /// Does `founder` with `salt` derive `id` under this kind's domain?
    #[must_use]
    pub fn derives(
        self,
        id: &[u8; 32],
        founder: &AccountId,
        salt: &[u8; NAMESPACE_SALT_LEN],
    ) -> bool {
        match self {
            Self::Ordinary => is_founded_by(id, founder, salt),
            Self::AccountNamespace => is_account_namespace_of(id, founder, salt),
        }
    }
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

    /// Record that `founder` founded `namespace_id` with `salt` through a `kind` genesis.
    /// Refused unless the pair derives the id under that kind; idempotent, never overwritten.
    ///
    /// # Errors
    /// A pair that does not reproduce the id, a conflicting record, or the
    /// store read/write.
    pub fn record(
        &self,
        namespace_id: &ContextGroupId,
        founder: &AccountId,
        salt: &[u8; NAMESPACE_SALT_LEN],
        kind: GenesisKind,
    ) -> EyreResult<()> {
        if !kind.derives(&namespace_id.to_bytes(), founder, salt) {
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

    /// The founder and salt `namespace_id` was derived from.
    ///
    /// `None` until this node has applied the namespace's genesis.
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

    /// Drop both records of a namespace this node no longer holds.
    ///
    /// # Errors
    /// The store write.
    pub fn forget(&self, namespace_id: &ContextGroupId) -> EyreResult<()> {
        let mut handle = self.store.handle();
        handle.delete(&key(namespace_id))?;
        handle.delete(&relay_key(namespace_id))?;
        Ok(())
    }
}

/// Namespaces whose genesis marked them as their founder's account namespace
/// (`RootOp::AccountNamespaceCreated`), recorded by that apply on every replica.
const ACCOUNT_SCOPE: [u8; 16] = *b"calimero-nsacctn";

fn account_key(namespace_id: &ContextGroupId) -> GenericKey {
    GenericKey::new(ACCOUNT_SCOPE, namespace_id.to_bytes())
}

impl NamespaceFoundingRepository<'_> {
    /// Mark `namespace_id` as its founder's account namespace.
    pub fn mark_account_namespace(&self, namespace_id: &ContextGroupId) -> EyreResult<()> {
        let data = GenericData::from(Slice::from(vec![1u8]));
        self.store.handle().put(&account_key(namespace_id), &data)?;
        Ok(())
    }

    /// The founder whose account namespace `namespace_id` is, or `None` when its
    /// genesis marked no account namespace (or has not applied here).
    pub fn account_namespace_founder(
        &self,
        namespace_id: &ContextGroupId,
    ) -> EyreResult<Option<AccountId>> {
        if !self.store.handle().has(&account_key(namespace_id))? {
            return Ok(None);
        }
        Ok(self.get(namespace_id)?.map(|(founder, _)| founder))
    }
}

/// The relay a namespace was founded through, and whether it has attested.
///
/// A namespace founded on a member's behalf has no admin node to admit a TEE,
/// so the relay the founder named in the genesis warrant is the one account
/// allowed to admit itself as the first TEE (`GroupOp::FoundingRelayAttested`),
/// once. Recorded by the delegated genesis apply, identically on every peer.
const RELAY_SCOPE: [u8; 16] = *b"calimero-nsrelay";

const RELAY_RECORD_LEN: usize = 32 + 1;

fn relay_key(namespace_id: &ContextGroupId) -> GenericKey {
    GenericKey::new(RELAY_SCOPE, namespace_id.to_bytes())
}

impl NamespaceFoundingRepository<'_> {
    /// Record `relay` as the relay `namespace_id` was founded through. A
    /// namespace has one genesis, so one founding relay: a different one is
    /// refused, the same one again is a no-op.
    pub fn record_founding_relay(
        &self,
        namespace_id: &ContextGroupId,
        relay: &AccountId,
    ) -> EyreResult<()> {
        if let Some((existing, _)) = self.founding_relay(namespace_id)? {
            if existing == *relay {
                return Ok(());
            }
            bail!("refusing to replace the founding relay recorded for namespace {namespace_id:?}");
        }
        self.put_relay(namespace_id, relay, false)
    }

    /// The relay `namespace_id` was founded through, and whether it has
    /// attested yet. `None` for a namespace founded by a node of its own.
    pub fn founding_relay(
        &self,
        namespace_id: &ContextGroupId,
    ) -> EyreResult<Option<(AccountId, bool)>> {
        let handle = self.store.handle();
        let Some(data) = handle.get(&relay_key(namespace_id))? else {
            return Ok(None);
        };
        let bytes: &[u8] = data.as_ref();
        if bytes.len() != RELAY_RECORD_LEN {
            bail!(
                "founding-relay record for namespace {namespace_id:?} is {} bytes, expected \
                 {RELAY_RECORD_LEN}",
                bytes.len()
            );
        }
        let mut account = [0u8; 32];
        account.copy_from_slice(&bytes[..32]);
        Ok(Some((AccountId::from(account), bytes[32] != 0)))
    }

    /// Mark the founding relay as having attested, so it cannot do it twice.
    pub fn mark_founding_relay_attested(&self, namespace_id: &ContextGroupId) -> EyreResult<()> {
        let Some((relay, _)) = self.founding_relay(namespace_id)? else {
            bail!("namespace {namespace_id:?} was not founded through a relay");
        };
        self.put_relay(namespace_id, &relay, true)
    }

    fn put_relay(
        &self,
        namespace_id: &ContextGroupId,
        relay: &AccountId,
        attested: bool,
    ) -> EyreResult<()> {
        let mut bytes = Vec::with_capacity(RELAY_RECORD_LEN);
        bytes.extend_from_slice(relay.as_bytes());
        bytes.push(u8::from(attested));
        let data = GenericData::from(Slice::from(bytes));
        self.store.handle().put(&relay_key(namespace_id), &data)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use calimero_account::{account_namespace_id, founded_namespace_id};

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
        repo.record(&derived(), &founder(), &SALT, GenesisKind::Ordinary)
            .unwrap();
        assert_eq!(repo.get(&derived()).unwrap(), Some((founder(), SALT)));
    }

    #[test]
    fn recording_the_same_pair_again_is_a_no_op() {
        let store = test_store();
        let repo = NamespaceFoundingRepository::new(&store);
        repo.record(&derived(), &founder(), &SALT, GenesisKind::Ordinary)
            .unwrap();
        repo.record(&derived(), &founder(), &SALT, GenesisKind::Ordinary)
            .unwrap();
        assert_eq!(repo.get(&derived()).unwrap(), Some((founder(), SALT)));
    }

    #[test]
    fn a_pair_that_does_not_derive_the_id_is_refused() {
        let store = test_store();
        let repo = NamespaceFoundingRepository::new(&store);
        let random = ContextGroupId::from([0x5a; 32]);
        assert!(repo
            .record(&random, &founder(), &SALT, GenesisKind::Ordinary)
            .is_err());
        assert!(repo
            .record(
                &derived(),
                &AccountId::from([0x12; 32]),
                &SALT,
                GenesisKind::Ordinary
            )
            .is_err());
        assert_eq!(repo.get(&random).unwrap(), None);
        assert_eq!(repo.get(&derived()).unwrap(), None);
    }

    #[test]
    fn a_pair_is_refused_under_the_other_genesis_kind() {
        let store = test_store();
        let repo = NamespaceFoundingRepository::new(&store);
        let account = ContextGroupId::from(account_namespace_id(&founder(), &SALT));
        assert!(repo
            .record(&derived(), &founder(), &SALT, GenesisKind::AccountNamespace)
            .is_err());
        assert!(repo
            .record(&account, &founder(), &SALT, GenesisKind::Ordinary)
            .is_err());
        repo.record(&account, &founder(), &SALT, GenesisKind::AccountNamespace)
            .unwrap();
        assert_eq!(repo.get(&derived()).unwrap(), None);
    }

    #[test]
    fn records_are_per_namespace() {
        let store = test_store();
        let repo = NamespaceFoundingRepository::new(&store);
        repo.record(&derived(), &founder(), &SALT, GenesisKind::Ordinary)
            .unwrap();
        let other = ContextGroupId::from(founded_namespace_id(&founder(), &[0x23; 32]));
        assert_eq!(repo.get(&other).unwrap(), None);
    }
}
