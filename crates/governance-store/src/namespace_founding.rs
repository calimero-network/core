//! What a derived namespace id was derived from: its founder and salt.
//!
//! A namespace root created without a caller-chosen id gets
//! `calimero_account::founded_namespace_id(founder, salt)` as its id, and its
//! genesis (`RootOp::NamespaceCreatedV2`) carries the salt (see
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
//! A joiner also writes it BEFORE it has any governance state, from the pair
//! its invitation carries ([`NamespaceFoundingRepository::adopt_invitation_hint`]).
//! That is what lets the genesis apply refuse a legacy `NamespaceCreated` for a
//! derived id on a bare replica (#2932): the row is the only thing telling it
//! the id was derived. The pair is unsigned and taken only if it derives the id.
//!
//! # Why a local row, not folded state
//!
//! It records a fact the genesis already carries, so it is hash-neutral like
//! the deny-list: every replica that applies the same genesis derives the same
//! row. A namespace founded before derivation has none.
//!
//! The one apply that reads it is genesis itself, and only to refuse a legacy
//! `NamespaceCreated` where a row exists. That keeps replicas convergent: a row
//! exists only for a pair that derives the id, exactly one pair does, and the
//! real genesis for such an id is a `NamespaceCreatedV2` naming that pair — so
//! the row refuses only forgeries, never the genesis every other replica
//! applies. A replica without a row keeps the legacy rules.
//!
//! # Why the founder is stored with the salt
//!
//! A pair that does not reproduce the id proves nothing, so
//! [`NamespaceFoundingRepository::record`] refuses one — a stored row always
//! verifies. Storing the founder rather than re-deriving it also keeps the row
//! right on the founding node after an account import changes its current
//! account.
//!
//! Neither half is a secret. Disclosing the pair grants nothing: the id commits
//! to the founder, so the salt cannot be replayed for another account.

use calimero_account::{is_founded_by, AccountId, NAMESPACE_SALT_LEN};
use calimero_context_config::types::{ContextGroupId, NamespaceFoundingHint};
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

/// Read/write access to the founding records of derived namespace ids.
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

    /// The pair to put in an invitation into `namespace_id`, if this node
    /// holds one ([`SignedGroupOpenInvitation::founding`]).
    ///
    /// [`SignedGroupOpenInvitation::founding`]: calimero_context_config::types::SignedGroupOpenInvitation::founding
    ///
    /// # Errors
    /// As [`Self::get`].
    pub fn invitation_hint(
        &self,
        namespace_id: &ContextGroupId,
    ) -> EyreResult<Option<Box<NamespaceFoundingHint>>> {
        Ok(self
            .get(namespace_id)?
            .map(|(founder, salt)| Box::new(NamespaceFoundingHint { founder, salt })))
    }

    /// Record the pair an invitation carried, before the joiner syncs
    /// anything, so its genesis apply refuses a legacy genesis for a derived
    /// id (#2932).
    ///
    /// The hint is unsigned, so it is taken only if it proves itself: a pair
    /// that does not derive `namespace_id` is ignored, never recorded, and
    /// leaves the join on the legacy rules. Returns whether it was recorded.
    ///
    /// # Errors
    /// The store read/write, or a different pair already recorded for this id
    /// (which would mean a SHA-256 collision, or a corrupt row).
    pub fn adopt_invitation_hint(
        &self,
        namespace_id: &ContextGroupId,
        hint: &NamespaceFoundingHint,
    ) -> EyreResult<bool> {
        if !hint.founds(&namespace_id.to_bytes()) {
            return Ok(false);
        }
        self.record(namespace_id, &hint.founder, &hint.salt)?;
        Ok(true)
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
    fn an_invitation_hint_that_derives_the_id_is_recorded() {
        let store = test_store();
        let repo = NamespaceFoundingRepository::new(&store);
        let hint = NamespaceFoundingHint {
            founder: founder(),
            salt: SALT,
        };
        assert!(repo.adopt_invitation_hint(&derived(), &hint).unwrap());
        assert_eq!(
            repo.invitation_hint(&derived()).unwrap().as_deref(),
            Some(&hint)
        );
    }

    /// The hint is unsigned, so a relayer can put anything there. A pair that
    /// does not derive the id is ignored — not an error that fails the join,
    /// and never a row the genesis gate would then read.
    #[test]
    fn an_invitation_hint_that_does_not_derive_the_id_is_never_recorded() {
        let store = test_store();
        let repo = NamespaceFoundingRepository::new(&store);
        let wrong_founder = NamespaceFoundingHint {
            founder: AccountId::from([0x12; 32]),
            salt: SALT,
        };
        let wrong_salt = NamespaceFoundingHint {
            founder: founder(),
            salt: [0x23; 32],
        };
        assert!(!repo
            .adopt_invitation_hint(&derived(), &wrong_founder)
            .unwrap());
        assert!(!repo.adopt_invitation_hint(&derived(), &wrong_salt).unwrap());
        let random = ContextGroupId::from([0x5a; 32]);
        let real = NamespaceFoundingHint {
            founder: founder(),
            salt: SALT,
        };
        assert!(!repo.adopt_invitation_hint(&random, &real).unwrap());
        assert_eq!(repo.get(&derived()).unwrap(), None);
        assert_eq!(repo.get(&random).unwrap(), None);
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
