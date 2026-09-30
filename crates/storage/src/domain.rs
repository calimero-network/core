//! The write rule a collection inherits from what holds it.
//!
//! An entry's stamp (`StorageType`) is what every node checks when it applies a
//! peer's write. A collection *inside* that entry's value is a separate entity
//! whose entries carry their own stamps, so without more it is `Public`: an
//! `AuthoredMap<String, UnorderedMap<..>>` owns its entries but not the maps in
//! them, and any member could write into those.
//!
//! A [`Domain`] closes that. It is in-memory state on a collection's
//! [`Element`](crate::entities::Element), never stored or sent, derived from the
//! only thing every node already verifies: the stamp of the entity the
//! collection was loaded from, or the policy of the wrapper that owns it.
//!
//! * **Loading.** [`Interface::find_by_id`](crate::interface::Interface::find_by_id)
//!   deserializes an entry with the ambient domain set from that entry's stamp,
//!   so every collection inside the value comes back in the entry's domain, at
//!   any depth: an entry of a nested collection is itself stamped in the domain,
//!   and loading it passes the domain one level further down.
//! * **Writing.** Inserting into a collection in a guarded domain stamps the
//!   entry for that domain, and an account without authority for it is refused
//!   before anything is written.
//! * **Reading.** A collection in a guarded domain ignores every entry whose
//!   stamp the domain does not admit. A patched peer can still send a `Public`
//!   entry that claims a guarded collection as its parent, and nodes store it,
//!   but none of them ever returns it. Entries the domain does admit carry a
//!   stamp every node verified on apply (an owner's signature, or a writer's
//!   against the anchor), so they cannot be forged.
//!
//! The collection container itself keeps its own stamp. Only its entries are
//! guarded, so honest nodes never disagree about the container.

use core::cell::{Cell, RefCell};

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_account::AccountId;

use crate::address::Id;
use crate::entities::{EntryRules, StorageType};
use crate::interface::StorageError;

/// Who may write a collection's entries, as inherited from what holds it.
///
/// Never part of an entity's bytes (`Element` skips it). Borsh encodes it only
/// for the node-local count of what a collection admits (`admitted_count`).
#[derive(
    BorshDeserialize, BorshSerialize, Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd,
)]
pub enum Domain {
    /// No inherited rule: entries keep the stamp they are written with.
    #[default]
    Open,
    /// Every entry must be owner-stamped, by any owner, with exactly these
    /// rules: the entries of an `Authored`, `WriteOnce` or `Moderated`
    /// collection, `UserStorage` or `AuthoredVector`.
    Owned(EntryRules),
    /// Inside an entry owned by this account: every entry is owned by it too.
    OwnedBy(AccountId),
    /// Inside a writer-set cell: every entry is a member of this anchor.
    Anchor(Id),
    /// Every entry must be frozen: `ContentAddressed<C>` and `FrozenStorage`.
    ContentAddressed,
    /// Inside frozen data. A frozen value's bytes are its identity, so a
    /// collection in it can hold nothing.
    Sealed,
}

impl Domain {
    /// The domain a collection inherits from the stamp of the entity it was
    /// loaded from or written into.
    #[must_use]
    pub fn inherited_from(stamp: &StorageType) -> Self {
        match stamp {
            // A written-once entry's value is fixed, so nothing inside it can
            // change either.
            StorageType::User { rules, .. } if rules.immutable => Self::Sealed,
            StorageType::User { owner, .. } => Self::OwnedBy(*owner),
            StorageType::SharedMember { anchor, .. } => Self::Anchor(*anchor),
            StorageType::Frozen => Self::Sealed,
            StorageType::Public | StorageType::Shared { .. } => Self::Open,
        }
    }

    /// Whether this domain is set by the policy of a wrapper (`Authored`,
    /// `Frozen`, `UserStorage`, …) rather than inherited from what holds it.
    #[must_use]
    pub const fn is_policy(&self) -> bool {
        matches!(self, Self::Owned(_) | Self::ContentAddressed)
    }

    /// Whether this domain carries a rule at all.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        matches!(self, Self::Open)
    }

    /// Whether an entry stamped `stamp` belongs to a collection in this domain.
    #[must_use]
    pub fn admits(&self, stamp: &StorageType) -> bool {
        match (self, stamp) {
            (Self::Open, _) | (Self::ContentAddressed, StorageType::Frozen) => true,
            (Self::Owned(rules), StorageType::User { rules: stamped, .. }) => rules == stamped,
            (Self::OwnedBy(owner), StorageType::User { owner: stamped, .. }) => owner == stamped,
            (
                Self::Anchor(anchor),
                StorageType::SharedMember {
                    anchor: stamped, ..
                },
            ) => anchor == stamped,
            _ => false,
        }
    }

    /// The stamp an entry written into a collection in this domain gets, given
    /// the stamp the write asked for.
    ///
    /// # Errors
    /// `ActionNotAllowed` for a sealed domain, which holds nothing.
    pub fn stamp_for(&self, requested: StorageType) -> Result<StorageType, StorageError> {
        match self {
            Self::Open | Self::Owned(_) | Self::ContentAddressed => Ok(requested),
            Self::OwnedBy(owner) => Ok(StorageType::User {
                rules: crate::entities::EntryRules::OWNED,
                owner: *owner,
                signature_data: None,
            }),
            Self::Anchor(anchor) => Ok(StorageType::SharedMember {
                anchor: *anchor,
                signature_data: None,
            }),
            Self::Sealed => Err(StorageError::ActionNotAllowed(
                "a collection inside frozen data cannot hold entries".to_owned(),
            )),
        }
    }
}

thread_local! {
    static AMBIENT: RefCell<Domain> = const { RefCell::new(Domain::Open) };
    /// Set while a value's nested collections are re-keyed into the entry that
    /// will hold it.
    static REKEYING: Cell<u32> = const { Cell::new(0) };
    /// Set when that re-key met an entry a sealed domain cannot hold.
    static SEALED_REFUSED: Cell<bool> = const { Cell::new(false) };
}

/// The domain collections take while they are deserialized or re-keyed.
pub(crate) fn ambient() -> Domain {
    AMBIENT.with(|a| a.borrow().clone())
}

/// Run `f` with the ambient domain set to `domain`, restoring the previous one
/// afterwards, including on unwind.
pub(crate) fn with_ambient<R>(domain: Domain, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Domain>);
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Some(previous) = self.0.take() {
                AMBIENT.with(|a| *a.borrow_mut() = previous);
            }
        }
    }
    let previous = AMBIENT.with(|a| core::mem::replace(&mut *a.borrow_mut(), domain));
    let _restore = Restore(Some(previous));
    f()
}

/// Re-key a value's nested collections with `f`, in the domain of the entry
/// the value is about to be stored in.
///
/// A frozen entry's value cannot hold a collection with entries: its bytes are
/// its identity, and entries are separate entities. The re-key would have to
/// write them, so each such write is skipped and remembered instead, and this
/// returns an error before the entry itself is stored.
///
/// # Errors
/// `ActionNotAllowed` if the value holds entries a sealed domain refuses.
pub(crate) fn rekey_into(domain: Domain, f: impl FnOnce()) -> Result<(), StorageError> {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            REKEYING.with(|r| r.set(r.get().saturating_sub(1)));
            SEALED_REFUSED.with(|s| s.set(self.0));
        }
    }
    let _restore = Restore(SEALED_REFUSED.with(|s| s.replace(false)));
    REKEYING.with(|r| r.set(r.get().saturating_add(1)));
    with_ambient(domain, f);
    if SEALED_REFUSED.with(Cell::get) {
        return Err(StorageError::ActionNotAllowed(
            "a frozen value cannot hold a collection with entries".to_owned(),
        ));
    }
    Ok(())
}

/// Whether a write into a sealed domain should be skipped rather than refused:
/// during a re-key it is skipped and reported by [`rekey_into`].
pub(crate) fn skip_sealed_write() -> bool {
    if REKEYING.with(Cell::get) == 0 {
        return false;
    }
    SEALED_REFUSED.with(|s| s.set(true));
    true
}

/// Deserialize a policy wrapper's inner collection with its entries admitted
/// only when owner-stamped. For `#[borsh(deserialize_with)]`.
pub(crate) fn deserialize_owned_entries<R, T>(reader: &mut R) -> borsh::io::Result<T>
where
    R: borsh::io::Read,
    T: borsh::BorshDeserialize + PolicyTarget,
{
    deserialize_in(reader, Domain::Owned(EntryRules::OWNED))
}

/// Deserialize a policy wrapper's inner collection with its entries admitted
/// only when frozen. For `#[borsh(deserialize_with)]`.
pub(crate) fn deserialize_frozen_entries<R, T>(reader: &mut R) -> borsh::io::Result<T>
where
    R: borsh::io::Read,
    T: borsh::BorshDeserialize + PolicyTarget,
{
    deserialize_in(reader, Domain::ContentAddressed)
}

/// A collection a policy wrapper holds its entries in.
pub(crate) trait PolicyTarget {
    /// Set the domain this collection's entries are admitted under.
    fn set_domain(&mut self, domain: Domain);
}

impl<T: crate::entities::Data> PolicyTarget for T {
    fn set_domain(&mut self, domain: Domain) {
        self.element_mut().domain = domain;
    }
}

/// `inner` with its entries admitted under a wrapper's policy `domain`.
pub(crate) fn with_policy<T: PolicyTarget>(mut inner: T, domain: Domain) -> T {
    inner.set_domain(domain);
    inner
}

fn deserialize_in<R, T>(reader: &mut R, domain: Domain) -> borsh::io::Result<T>
where
    R: borsh::io::Read,
    T: borsh::BorshDeserialize + PolicyTarget,
{
    Ok(with_policy(T::deserialize_reader(reader)?, domain))
}

/// Refuse a local write to an entry in `domain` by an account without authority
/// for it. A no-op for open domains and during a merge, which replays writes
/// other nodes already verified.
///
/// The check that makes the rule hold is the one every node runs on apply; this
/// one keeps an honest node from storing a write every other node will refuse.
///
/// A writer-set cell's members are not checked here. Their writes are stamped
/// `SharedMember`, so every other node checks them against the anchor's writer
/// set on apply, and a cell has always left that check to apply: a non-writer's
/// write lands on its own node and nowhere else, which is what the
/// `SharedStorage` scenarios show on real nodes.
///
/// # Errors
/// `ActionNotAllowed` when the calling account may not make this write.
pub(crate) fn check_authority(domain: &Domain) -> Result<(), StorageError> {
    if crate::env::in_merge_mode() {
        return Ok(());
    }
    match domain {
        Domain::Open | Domain::Owned(_) | Domain::ContentAddressed | Domain::Anchor(_) => Ok(()),
        Domain::OwnedBy(owner) if *owner == AccountId::from(crate::env::account_id()) => Ok(()),
        Domain::OwnedBy(_) => Err(StorageError::ActionNotAllowed(
            "only the owner of the enclosing entry may change it".to_owned(),
        )),
        Domain::Sealed => Err(StorageError::ActionNotAllowed(
            "frozen data cannot be changed".to_owned(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alice() -> AccountId {
        AccountId::from([0x11; 32])
    }

    fn bob() -> AccountId {
        AccountId::from([0x22; 32])
    }

    fn user(owner: AccountId) -> StorageType {
        StorageType::User {
            rules: crate::entities::EntryRules::OWNED,
            owner,
            signature_data: None,
        }
    }

    fn member(anchor: Id) -> StorageType {
        StorageType::SharedMember {
            anchor,
            signature_data: None,
        }
    }

    #[test]
    fn each_stamp_passes_its_rule_down() {
        let anchor = Id::new([7; 32]);
        assert_eq!(Domain::inherited_from(&StorageType::Public), Domain::Open);
        assert_eq!(
            Domain::inherited_from(&user(alice())),
            Domain::OwnedBy(alice())
        );
        assert_eq!(
            Domain::inherited_from(&member(anchor)),
            Domain::Anchor(anchor)
        );
        assert_eq!(Domain::inherited_from(&StorageType::Frozen), Domain::Sealed);
    }

    #[test]
    fn a_domain_admits_exactly_its_own_stamps() {
        let anchor = Id::new([7; 32]);
        let other = Id::new([8; 32]);
        let stamps = [
            StorageType::Public,
            StorageType::Frozen,
            user(alice()),
            user(bob()),
            member(anchor),
            member(other),
        ];
        let admitted = |domain: Domain| -> Vec<bool> {
            stamps.iter().map(|stamp| domain.admits(stamp)).collect()
        };
        assert_eq!(admitted(Domain::Open), [true; 6]);
        assert_eq!(
            admitted(Domain::Owned(EntryRules::OWNED)),
            [false, false, true, true, false, false]
        );
        assert_eq!(
            admitted(Domain::OwnedBy(alice())),
            [false, false, true, false, false, false]
        );
        assert_eq!(
            admitted(Domain::Anchor(anchor)),
            [false, false, false, false, true, false]
        );
        assert_eq!(
            admitted(Domain::ContentAddressed),
            [false, true, false, false, false, false]
        );
        assert_eq!(admitted(Domain::Sealed), [false; 6]);
    }

    #[test]
    fn a_write_is_stamped_for_the_domain_it_lands_in() {
        let anchor = Id::new([7; 32]);
        assert_eq!(
            Domain::OwnedBy(alice())
                .stamp_for(StorageType::Public)
                .unwrap(),
            user(alice())
        );
        assert_eq!(
            Domain::Anchor(anchor)
                .stamp_for(StorageType::Public)
                .unwrap(),
            member(anchor)
        );
        assert_eq!(
            Domain::Owned(EntryRules::OWNED)
                .stamp_for(user(bob()))
                .unwrap(),
            user(bob()),
            "a policy's own stamp is kept"
        );
        assert!(Domain::Sealed.stamp_for(StorageType::Public).is_err());
        // Every stamp a domain writes, it admits.
        for domain in [Domain::OwnedBy(alice()), Domain::Anchor(anchor)] {
            let stamp = domain.stamp_for(StorageType::Public).unwrap();
            assert!(domain.admits(&stamp), "{domain:?} must admit {stamp:?}");
        }
    }

    #[test]
    fn the_ambient_domain_nests_and_restores() {
        assert_eq!(ambient(), Domain::Open);
        with_ambient(Domain::OwnedBy(alice()), || {
            assert_eq!(ambient(), Domain::OwnedBy(alice()));
            with_ambient(Domain::Sealed, || assert_eq!(ambient(), Domain::Sealed));
            assert_eq!(ambient(), Domain::OwnedBy(alice()));
        });
        assert_eq!(ambient(), Domain::Open);
        let unwound = std::panic::catch_unwind(|| {
            with_ambient(Domain::Sealed, || panic!("unwind"));
        });
        assert!(unwound.is_err());
        assert_eq!(ambient(), Domain::Open, "restored on unwind");
    }
}
