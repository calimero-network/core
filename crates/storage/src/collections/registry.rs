//! Names with at most one owner each, decided by an authority.
//!
//! No rule a leaderless replica can apply on its own gives a name a single
//! owner. Two members on either side of a partition who claim `alice` see the
//! same local state whether or not the other claim exists, so whatever lets one
//! of them call the name theirs lets the other do the same. "Earliest claim
//! wins" is worse than that: apply bounds a write's timestamp only from above
//! (`verify_action_timestamp`), so a patched node claims with a timestamp of 1
//! and takes any name, including one held for years.
//!
//! So a [`Registry`] keeps two things apart: the **claims** members make, and
//! the **verdicts** an authority `A` writes about them. Nothing is owned until
//! there is a verdict.
//!
//! ```ignore
//! names: Registry<String, Profile>,              // the TEE decides (the default)
//! names: Registry<String, Profile, Admin>,       // the admins decide
//! names: Registry<String, Profile, NoAuthority>, // nobody decides: contests are reported
//!
//! self.names.claim(name.clone(), profile)?;      // any member, then fire the resolver
//! self.names.resolve(&name)?;                    // the authority only
//! self.names.status(&name)?;                     // Free / Pending / Owned / Lost / Contested
//! self.names.owner_of(&name)?;                   // Some only once a verdict names an owner
//! ```
//!
//! # Layout
//!
//! * `claims` is an [`Authored`]`<UnorderedMap<K, Claim<V>>>`. A claim is its
//!   claimant's own entry, at the id derived from the key AND the owner, so
//!   nobody can write, edit or withdraw anyone else's, and the authority reads
//!   every claimant of a name in one trie-bucket read (`entries_at`). `Authored`
//!   rather than `WriteOnce`: a claim is edited when its owner re-claims at a
//!   later epoch or asks for a release.
//! * `verdicts` is the authority's cell ([`RegistryAuthority::Verdicts`]): a
//!   `TeeOnly` or `SharedStorage` [`SortedMap`] every node refuses a write to
//!   unless its signer is in the cell's writer set. The existing `TeeOnly` and
//!   `Shared` apply rules are the whole enforcement; a verdict needs no rule of
//!   its own.
//!
//! # The verdict rule
//!
//! Each verdict is its own entry, keyed by [`VerdictKey`]:
//! `H(name) ‖ !epoch ‖ vacant ‖ order ‖ by`, so a name's verdicts share a prefix
//! and sort best first: the highest epoch, a grant before a vacancy, then the
//! lowest `order`. The verdicts of a name are a grow-only set, and its standing
//! is a function of that set ([`RegistryAuthority::standing`]; by default the
//! first well-formed entry), so replicas holding the same verdicts agree on it
//! however they arrived, and a stale or rolled-back authority's verdict, at a
//! lower epoch or a higher order, can only lose.
//!
//! One entry per verdict, not one per name merged by a custom rule, so the
//! standing is decided by the keys alone and never depends on a merge running.
//! Distinct ids never meet at merge, and the key ends in the writing device, so
//! two authorities that reach the same decision write two entries side by side
//! rather than one that last-write-wins. (A per-name cell merged by a custom
//! rule once split nodes by delivery order, because apply dropped a
//! `SharedMember` write whose nonce was below the stored one before the merge
//! ran; apply now merges such a write, `tests/converge_signed_mergeable.rs`.)
//! [`Verdict`]'s `Mergeable` is the same maximum, for the root-merge path.
//!
//! # Epochs
//!
//! A name's epoch counts its releases. A claim bids for the name's open epoch:
//! 0 until the first verdict, the vacancy's epoch after a release. A grant at
//! epoch `e` names an owner; the owner asks to let go ([`Registry::release`])
//! by marking its claim, and the authority answers with a vacancy at `e + 1`.
//! Claims for an earlier epoch are stale and never considered again, so after
//! a release everyone claims afresh.
//!
//! A verdict counts only once the claim it names has reached this node: a
//! grant whose claim is still syncing reads as pending, which also means the
//! authority cannot hand a name to an account that never asked for it.
//! The owner's own node can still remove that claim (it is the owner's entry);
//! the name then reads as pending and nobody else can take it, which is no more
//! than keeping it would have done.
//!
//! # Who wins among claimants
//!
//! The authority resolves a name as soon as a claim for it reaches the
//! authority, so in practice the first claim to reach it wins. Several claims
//! it sees at once are split by the lowest `order`:
//! `order = H(claim_ref ‖ vacant)`, `claim_ref = H(name ‖ epoch ‖ owner)`.
//! That is:
//!
//! * **independent of every clock.** No self-reported timestamp enters it, so
//!   backdating buys nothing, and a host that rewinds its TEE's clock cannot
//!   make a verdict sort earlier.
//! * **computable by every node.** Every input is in state, so a reader
//!   recomputes `claim_ref` and `order` and ignores a verdict that does not
//!   match its key. A trigger's delta id would do as well for ordering, but no
//!   reader could check it, since app code cannot see the DAG.
//! * **the same as the merge.** Two authorities that saw different claimants
//!   pick different owners; the lower order wins the merge, which is the owner
//!   one authority holding both claims would have picked.
//! * **grindable only with accounts.** The claim's bytes do not enter the
//!   order, so moving one's rank takes another admitted account.
//!
//! # Authorities
//!
//! | `A` | who writes verdicts | how resolution runs | final | offline authority |
//! |---|---|---|---|---|
//! | [`Tee`] (default) | [`AccountId::TEE_AUTHORITY`], through a `TeeOnly` cell | an `#[app::tee]` method fired by the claim's `tee:` event, plus an `#[app::tee(every = ..)]` sweep of [`resolve_all_pending`](Registry::resolve_all_pending) | at the verdict, with one TEE authority | claims stay pending |
//! | [`Admin`] | the admins, a rotatable `SharedStorage` writer set | an admin calls [`resolve`](Registry::resolve) | at the verdict, with one admin device | claims stay pending |
//! | [`NoAuthority`] | nobody | never | never: nobody owns anything | n/a |
//!
//! With several TEEs (failover fires at least once, on each), or several admin
//! devices resolving concurrently, two verdicts can name different owners at
//! one epoch. Both converge on the lower order, and until they meet each side
//! believes its own. `Status::Owned`'s `stable` turns false once a node holds
//! a rival grant, but a node that has not heard of the rival cannot know.
//!
//! # Adding an authority
//!
//! Claim storage and the API do not change. An authority supplies:
//!
//! 1. **the writer set**: a [`VerdictStore`], a cell whose writes every node
//!    checks on apply. A moderator set is a `SharedStorage` cell like
//!    [`Admin`]'s with its own rotation policy.
//! 2. **how resolution is triggered** ([`RegistryAuthority::TRIGGER`]), which
//!    the app wires: an event handler, a timer, a UI button.
//! 3. **how the winner is picked** ([`RegistryAuthority::pick`]), **which
//!    verdict stands** ([`RegistryAuthority::standing`]) and **when it is
//!    final** ([`RegistryAuthority::stable`]).
//!
//! An M-of-N quorum (a council that votes) fits the same trait: its
//! `VerdictStore` is a `SharedStorage` cell whose writers are the council, a
//! member's verdict is its vote, and `standing` walks the name's verdicts best
//! first and returns the first claim that `q` distinct members granted at one
//! epoch. The key already keeps one entry per device, so two votes for one
//! claim never collapse into one. It gives true finality (two quorums
//! intersect, so at most one claim reaches `q`) with no single trusted party,
//! and it is live only while `q` members are online. What is missing for it:
//!
//! * a voter the count can trust: `by` is written by the voter, while the
//!   account the node resolved the entry's signature to lives in its stamp,
//!   which `standing` does not see yet;
//! * equivocation handling: one account with votes for two claims at one
//!   epoch (from two devices, say) must count for neither, not for whichever
//!   the read met first;
//! * the council fixed per epoch, so a rotation cannot re-count old votes;
//! * ballots, or a fallback authority, since a three-way split stalls a single
//!   round forever.

use core::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_account::AccountId;
use calimero_primitives::identity::PublicKey;
use sha2::{Digest, Sha256};

use super::crdt_meta::{MergeError, MergeStrategy, Mergeable};
use super::permissioned::{Op, SharedStorage, TeeAuthorityAcl, TeeOnly, WriterSetAcl};
use super::rekey::{field_child_id, RekeyTarget};
use super::{Authored, PermissionedStorage, SortedMap, StorageKey, StoreError, UnorderedMap};
use crate::address::Id;
use crate::env;
use crate::interface::StorageError;

mod sealed {
    pub trait Sealed {}
}

const NAME_DOMAIN: &[u8] = b"calimero.registry.name";
const CLAIM_DOMAIN: &[u8] = b"calimero.registry.claim";
const ORDER_DOMAIN: &[u8] = b"calimero.registry.order";

/// The length of a [`VerdictKey`].
pub const VERDICT_KEY_LEN: usize = 32 + 4 + 1 + 32 + 32;

/// Where a verdict is stored:
/// `H(name) ‖ (u32::MAX - epoch) ‖ vacant ‖ order ‖ by`, so a name's verdicts
/// share a prefix, sort best first, and one device's never overwrites
/// another's.
pub type VerdictKey = [u8; VERDICT_KEY_LEN];

/// Every verdict of one registry.
pub type Verdicts = SortedMap<VerdictKey, Verdict>;

/// A member's request for a name.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct Claim<V> {
    /// What the claimant attaches to the name.
    pub value: V,
    /// The epoch this claim bids for.
    pub epoch: u32,
    /// Set by the owner to ask the authority to free the name.
    pub release: bool,
}

/// An authority's decision about one name at one epoch.
#[derive(BorshSerialize, BorshDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Verdict {
    /// The epoch this verdict decides.
    pub epoch: u32,
    /// The owner it grants the name to, or `None` for a vacancy after a
    /// release.
    pub owner: Option<AccountId>,
    /// `H(name ‖ epoch ‖ owner)` of the claim it grants, or of the claim whose
    /// release it answers.
    pub claim_ref: [u8; 32],
    /// `H(claim_ref ‖ vacant)`: the tie-break between verdicts at one epoch.
    pub order: [u8; 32],
    /// The device that wrote it, as it says: a TEE's attested key, an admin's
    /// device. The last component of its key.
    pub by: PublicKey,
}

impl Verdict {
    fn grant(name: &[u8; 32], epoch: u32, owner: AccountId) -> Self {
        let claim_ref = claim_ref(name, epoch, &owner);
        Self {
            epoch,
            owner: Some(owner),
            claim_ref,
            order: order(&claim_ref, false),
            by: PublicKey::from(env::device_id()),
        }
    }

    /// The vacancy at `released_epoch + 1` answering `owner`'s release.
    fn vacancy(
        name: &[u8; 32],
        released_epoch: u32,
        owner: &AccountId,
    ) -> Result<Self, StoreError> {
        let epoch = released_epoch
            .checked_add(1)
            .ok_or_else(|| not_allowed("Registry::release: the name's epochs are exhausted"))?;
        let claim_ref = claim_ref(name, released_epoch, owner);
        Ok(Self {
            epoch,
            owner: None,
            claim_ref,
            order: order(&claim_ref, true),
            by: PublicKey::from(env::device_id()),
        })
    }

    /// Lower is better: a higher epoch, then a grant before a vacancy, then a
    /// lower order, then a lower writer. The key's bytes sort exactly as this
    /// does.
    fn rank(&self) -> (Reverse<u32>, bool, [u8; 32], PublicKey) {
        (
            Reverse(self.epoch),
            self.owner.is_none(),
            self.order,
            self.by,
        )
    }

    fn key(&self, name: &[u8; 32]) -> VerdictKey {
        let by: &[u8; 32] = self.by.as_ref();
        let mut key = [0; VERDICT_KEY_LEN];
        key[..32].copy_from_slice(name);
        key[32..36].copy_from_slice(&(u32::MAX - self.epoch).to_be_bytes());
        key[36] = u8::from(self.owner.is_none());
        key[37..69].copy_from_slice(&self.order);
        key[69..].copy_from_slice(by);
        key
    }

    /// Whether `order` and, for a grant, `claim_ref` are what every node would
    /// derive. A vacancy's `claim_ref` names an owner it does not carry, so only
    /// its order is checked.
    fn is_well_formed(&self, name: &[u8; 32]) -> bool {
        self.order == order(&self.claim_ref, self.owner.is_none())
            && self
                .owner
                .is_none_or(|owner| self.claim_ref == claim_ref(name, self.epoch, &owner))
    }
}

/// The better-ranked verdict wins. The rank is a total order ending in `by`,
/// so the merge is commutative, associative and idempotent, and it picks what
/// the default read rule picks.
#[diagnostic::do_not_recommend]
impl Mergeable for Verdict {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        if other.rank() < self.rank() {
            *self = *other;
        }
        Ok(())
    }
}

#[diagnostic::do_not_recommend]
impl RekeyTarget for Verdict {
    fn rekey_relative_to(&mut self, _parent_id: Id) {}
}

/// Structural: a verdict holds no collection and is never stamped for
/// dispatch, since its key names its writer and it is written once.
#[diagnostic::do_not_recommend]
impl MergeStrategy for Verdict {
    const DISPATCHED: bool = false;
}

/// Where a name stands, as the calling account sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Nobody holds or claims it.
    Free,
    /// Claimed and not yet decided, or granted to a claim that has not
    /// reached this node yet.
    Pending {
        /// How many accounts claim it.
        claimants: usize,
        /// Whether the caller is one of them.
        mine: bool,
    },
    /// A verdict names `owner`, and its claim has reached this node.
    Owned {
        /// The account that holds the name.
        owner: AccountId,
        /// The epoch the verdict decides.
        epoch: u32,
        /// Whether the authority counts the verdict final. For [`Tee`] and
        /// [`Admin`], false once this node holds a grant of another claim at
        /// the same epoch.
        stable: bool,
    },
    /// The caller claimed this epoch and `owner` was granted it.
    Lost {
        /// The account that holds the name.
        owner: AccountId,
        /// The epoch the verdict decides.
        epoch: u32,
    },
    /// No authority decides, and more than one account claims it.
    Contested {
        /// Every claimant.
        claimants: Vec<AccountId>,
    },
}

/// A claim the authority may grant, with the order it would be granted in.
#[derive(Debug)]
pub struct Candidate<'a, V> {
    /// The claimant.
    pub owner: AccountId,
    /// Its claim.
    pub claim: &'a Claim<V>,
    /// `H(claim_ref)`, the order a verdict granting it would carry.
    pub order: [u8; 32],
}

/// How an authority's resolution runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// An `#[app::tee]` method the app fires with a `tee:` event handler when a
    /// claim or release is made, backed by a timer sweep.
    TeeHandler,
    /// A call by a writer of the verdict cell.
    Explicit,
    /// Nothing resolves: there is no authority.
    Never,
}

/// The cell an authority writes verdicts into.
///
/// Sealed: a verdict is only as strong as the check every node runs when it
/// applies one, and those checks live in the storage layer.
pub trait VerdictStore: sealed::Sealed + BorshSerialize + BorshDeserialize + RekeyTarget {
    #[doc(hidden)]
    fn fresh() -> Self;
    #[doc(hidden)]
    fn read(&self) -> Result<Option<&Verdicts>, StoreError>;
    /// The verdicts, for a write the calling account is allowed to make.
    #[doc(hidden)]
    fn write(&mut self) -> Result<&mut Verdicts, StoreError>;
    #[doc(hidden)]
    fn may_write(&self) -> bool;
    #[doc(hidden)]
    fn reassign(&mut self, field_name: &str);
}

/// The verdict store of a registry with no authority: nothing, and no writes.
#[derive(Clone, Copy, Debug, BorshSerialize, BorshDeserialize)]
pub struct NoVerdicts;

/// Who decides a [`Registry`]'s names. See the [module documentation](self)
/// for what each one guarantees, and for how a new one plugs in.
pub trait RegistryAuthority: sealed::Sealed + 'static {
    /// Who may write verdicts, enforced on apply by every node.
    type Verdicts: VerdictStore;

    /// How resolution is triggered.
    const TRIGGER: Trigger;

    /// The index of the claim to grant, among claims that all bid for the
    /// name's open epoch. The lowest order, by default: see the [module
    /// documentation](self#who-wins-among-claimants).
    fn pick<V>(candidates: &[Candidate<'_, V>]) -> Option<usize> {
        candidates
            .iter()
            .enumerate()
            .min_by_key(|(_, candidate)| candidate.order)
            .map(|(at, _)| at)
    }

    /// Which of a name's well-formed verdicts stands, given all of them best
    /// ranked first. By default the first; an authority that counts votes
    /// returns the first claim with enough of them.
    fn standing(ranked: &[Verdict]) -> Option<usize> {
        (!ranked.is_empty()).then_some(0)
    }

    /// Whether the standing grant is final, given every well-formed verdict of
    /// its name. By default: no grant of another claim shares its epoch. With
    /// one writer there never is one.
    fn stable(standing: &Verdict, ranked: &[Verdict]) -> bool {
        !ranked.iter().any(|rival| {
            rival.epoch == standing.epoch
                && rival.owner.is_some()
                && rival.claim_ref != standing.claim_ref
        })
    }
}

/// Verdicts are written by an attested TEE, from `#[app::tee]` methods.
#[derive(Clone, Copy, Debug)]
pub struct Tee;

/// Verdicts are written by the admins, a writer set that starts as the
/// registry's creator and that an admin rotates.
#[derive(Clone, Copy, Debug)]
pub struct Admin;

/// Nobody writes verdicts: [`Registry::status`] reports contested names and
/// [`Registry::owner_of`] never names an owner.
#[derive(Clone, Copy, Debug)]
pub struct NoAuthority;

impl sealed::Sealed for Tee {}
impl sealed::Sealed for Admin {}
impl sealed::Sealed for NoAuthority {}
impl sealed::Sealed for TeeOnly<Verdicts> {}
impl sealed::Sealed for SharedStorage<Verdicts> {}
impl sealed::Sealed for NoVerdicts {}

impl RegistryAuthority for Tee {
    type Verdicts = TeeOnly<Verdicts>;
    const TRIGGER: Trigger = Trigger::TeeHandler;
}

impl RegistryAuthority for Admin {
    type Verdicts = SharedStorage<Verdicts>;
    const TRIGGER: Trigger = Trigger::Explicit;
}

impl RegistryAuthority for NoAuthority {
    type Verdicts = NoVerdicts;
    const TRIGGER: Trigger = Trigger::Never;
}

fn current_account() -> AccountId {
    AccountId::from(env::account_id())
}

fn not_allowed(message: &str) -> StoreError {
    StoreError::StorageError(StorageError::ActionNotAllowed(message.to_owned()))
}

impl VerdictStore for PermissionedStorage<Verdicts, TeeAuthorityAcl> {
    fn fresh() -> Self {
        Self::new_tee_only()
    }
    fn read(&self) -> Result<Option<&Verdicts>, StoreError> {
        self.try_get()
    }
    // `get_mut` refuses anyone but the TEE authority: `TeeAuthorityAcl` guards
    // in-place edits.
    fn write(&mut self) -> Result<&mut Verdicts, StoreError> {
        self.get_mut()
    }
    fn may_write(&self) -> bool {
        self.can(&current_account(), Op::Write)
    }
    fn reassign(&mut self, field_name: &str) {
        self.reassign_deterministic_id(field_name);
    }
}

impl VerdictStore for PermissionedStorage<Verdicts, WriterSetAcl> {
    /// The calling account is the first admin.
    fn fresh() -> Self {
        Self::new(BTreeSet::from([current_account()]), false)
    }
    fn read(&self) -> Result<Option<&Verdicts>, StoreError> {
        self.get().map(Some)
    }
    // `WriterSetAcl` leaves in-place edits to merge, so guard them here.
    fn write(&mut self) -> Result<&mut Verdicts, StoreError> {
        self.guard(Op::Write)?;
        self.get_mut()
    }
    fn may_write(&self) -> bool {
        self.can(&current_account(), Op::Write)
    }
    fn reassign(&mut self, field_name: &str) {
        self.reassign_deterministic_id(field_name);
    }
}

impl VerdictStore for NoVerdicts {
    fn fresh() -> Self {
        Self
    }
    fn read(&self) -> Result<Option<&Verdicts>, StoreError> {
        Ok(None)
    }
    fn write(&mut self) -> Result<&mut Verdicts, StoreError> {
        Err(not_allowed("Registry: this registry has no authority"))
    }
    fn may_write(&self) -> bool {
        false
    }
    fn reassign(&mut self, _field_name: &str) {}
}

#[diagnostic::do_not_recommend]
impl RekeyTarget for NoVerdicts {
    fn rekey_relative_to(&mut self, _parent_id: Id) {}
}

/// `H(name)`: a fixed-width prefix, so no name's verdicts sit under another's.
fn name_of(key: &[u8]) -> [u8; 32] {
    Sha256::new()
        .chain_update(NAME_DOMAIN)
        .chain_update(key)
        .finalize()
        .into()
}

fn claim_ref(name: &[u8; 32], epoch: u32, owner: &AccountId) -> [u8; 32] {
    Sha256::new()
        .chain_update(CLAIM_DOMAIN)
        .chain_update(name)
        .chain_update(epoch.to_be_bytes())
        .chain_update(owner.as_bytes())
        .finalize()
        .into()
}

fn order(claim_ref: &[u8; 32], vacant: bool) -> [u8; 32] {
    Sha256::new()
        .chain_update(ORDER_DOMAIN)
        .chain_update(claim_ref)
        .chain_update([u8::from(vacant)])
        .finalize()
        .into()
}

/// A name's standing verdict, and whether its authority counts it final.
struct Standing {
    verdict: Verdict,
    stable: bool,
}

/// The epoch a claim bids for: 0 before any verdict, else the standing
/// verdict's (a vacancy's, or a grant's whose claim is still syncing).
fn open_epoch(standing: Option<&Standing>) -> u32 {
    standing.map_or(0, |standing| standing.verdict.epoch)
}

/// The claimants bidding for `epoch` who have not asked to let go.
fn live<V>(
    claims: &[(AccountId, Claim<V>)],
    epoch: u32,
) -> impl Iterator<Item = &(AccountId, Claim<V>)> {
    claims
        .iter()
        .filter(move |(_, claim)| claim.epoch == epoch && !claim.release)
}

/// Unique names, each owned by one account, decided by the authority `A`.
///
/// See the [module documentation](self).
pub struct Registry<K, V, A: RegistryAuthority = Tee> {
    claims: Authored<UnorderedMap<K, Claim<V>>>,
    verdicts: A::Verdicts,
}

impl<K, V, A> BorshSerialize for Registry<K, V, A>
where
    K: StorageKey,
    V: BorshSerialize + BorshDeserialize + 'static,
    A: RegistryAuthority,
{
    fn serialize<W: borsh::io::Write>(&self, writer: &mut W) -> borsh::io::Result<()> {
        self.claims.serialize(writer)?;
        self.verdicts.serialize(writer)
    }
}

impl<K, V, A> BorshDeserialize for Registry<K, V, A>
where
    K: StorageKey,
    V: BorshSerialize + BorshDeserialize + 'static,
    A: RegistryAuthority,
{
    fn deserialize_reader<R: borsh::io::Read>(reader: &mut R) -> borsh::io::Result<Self> {
        Ok(Self {
            claims: BorshDeserialize::deserialize_reader(reader)?,
            verdicts: BorshDeserialize::deserialize_reader(reader)?,
        })
    }
}

impl<K, V, A> Registry<K, V, A>
where
    K: StorageKey,
    V: BorshSerialize + BorshDeserialize + 'static,
    A: RegistryAuthority,
{
    /// Creates an empty registry with random ids, created by the calling
    /// account: for [`Admin`], its first admin.
    ///
    /// Right for top-level `#[app::state]` fields: the macro reassigns
    /// deterministic ids after `init()`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            claims: Authored::new(),
            verdicts: A::Verdicts::fresh(),
        }
    }

    /// Reassigns both halves' ids deterministically from `field_name`. Called
    /// by the `#[app::state]` macro after `init()`.
    pub fn reassign_deterministic_id(&mut self, field_name: &str) {
        self.claims
            .reassign_deterministic_id(&format!("__registry_claims_{field_name}"));
        self.verdicts
            .reassign(&format!("__registry_verdicts_{field_name}"));
    }

    /// Claims `key` for the calling account at the name's open epoch,
    /// attaching `value`. Claiming again replaces the caller's claim.
    ///
    /// A claim decides nothing. For [`Tee`], fire the resolver: emit an event
    /// whose handler is the `#[app::tee]` method calling
    /// [`resolve`](Self::resolve).
    ///
    /// # Errors
    /// `ActionNotAllowed` if the name is owned, the caller's own included; any
    /// storage error.
    pub fn claim(&mut self, key: K, value: V) -> Result<(), StoreError> {
        let claims = self.claims.entries_at(&key)?;
        let standing = self.standing(&key)?;
        if let Some((owner, _)) = Self::owned(standing.as_ref(), &claims) {
            return Err(not_allowed(if owner == current_account() {
                "Registry::claim: the caller already owns this name"
            } else {
                "Registry::claim: another account owns this name"
            }));
        }
        let claim = Claim {
            value,
            epoch: open_epoch(standing.as_ref()),
            release: false,
        };
        if self.claims.contains(&key)? {
            self.claims.update(&key, claim)
        } else {
            self.claims.insert(key, claim)
        }
    }

    /// Withdraws the calling account's claim to `key`, returning whether it
    /// held one.
    ///
    /// # Errors
    /// `ActionNotAllowed` if the caller owns the name: an owner
    /// [`release`](Self::release)s it instead. Any storage error.
    pub fn withdraw(&mut self, key: &K) -> Result<bool, StoreError> {
        if self.owner_of(key)? == Some(current_account()) {
            return Err(not_allowed(
                "Registry::withdraw: the caller owns this name; release it instead",
            ));
        }
        Ok(self.claims.remove(key)?.is_some())
    }

    /// Asks the authority to free `key`, which the calling account owns. The
    /// name stays owned until the authority answers with a vacancy at the next
    /// epoch; for [`Tee`], fire the resolver as for a claim.
    ///
    /// # Errors
    /// `ActionNotAllowed` if the caller does not own the name; any storage
    /// error.
    pub fn release(&mut self, key: &K) -> Result<(), StoreError> {
        if self.owner_of(key)? != Some(current_account()) {
            return Err(not_allowed(
                "Registry::release: only the name's owner can release it",
            ));
        }
        self.claims.modify(key, |claim| claim.release = true)
    }

    /// Where `key` stands, as the calling account sees it.
    ///
    /// # Errors
    /// Any storage error.
    pub fn status(&self, key: &K) -> Result<Status, StoreError> {
        let me = current_account();
        let claims = self.claims.entries_at(key)?;
        if A::TRIGGER == Trigger::Never {
            return Ok(match claims.as_slice() {
                [] => Status::Free,
                [(owner, _)] => Status::Pending {
                    claimants: 1,
                    mine: *owner == me,
                },
                _ => Status::Contested {
                    claimants: claims.iter().map(|(owner, _)| *owner).collect(),
                },
            });
        }

        let standing = self.standing(key)?;
        if let (Some(standing), Some((owner, _))) =
            (standing.as_ref(), Self::owned(standing.as_ref(), &claims))
        {
            let epoch = standing.verdict.epoch;
            let lost = owner != me
                && claims
                    .iter()
                    .any(|(claimant, claim)| *claimant == me && claim.epoch == epoch);
            return Ok(if lost {
                Status::Lost { owner, epoch }
            } else {
                Status::Owned {
                    owner,
                    epoch,
                    stable: standing.stable,
                }
            });
        }

        let bidders: Vec<_> = live(&claims, open_epoch(standing.as_ref()))
            .map(|(owner, _)| *owner)
            .collect();
        let granted = standing
            .as_ref()
            .is_some_and(|standing| standing.verdict.owner.is_some());
        Ok(if bidders.is_empty() && !granted {
            Status::Free
        } else {
            Status::Pending {
                claimants: bidders.len(),
                mine: bidders.contains(&me),
            }
        })
    }

    /// The account that owns `key`: `Some` only once a verdict names it and
    /// its claim has reached this node. Gate every use of a name (a mention, a
    /// URL, a payment) on this, never on holding a claim.
    ///
    /// # Errors
    /// Any storage error.
    pub fn owner_of(&self, key: &K) -> Result<Option<AccountId>, StoreError> {
        let claims = self.claims.entries_at(key)?;
        let standing = self.standing(key)?;
        Ok(Self::owned(standing.as_ref(), &claims).map(|(owner, _)| owner))
    }

    /// The value the owner of `key` attached to its claim, once owned.
    ///
    /// # Errors
    /// Any storage error.
    pub fn value_of(&self, key: &K) -> Result<Option<V>, StoreError> {
        let claims = self.claims.entries_at(key)?;
        let standing = self.standing(key)?;
        let Some((owner, _)) = Self::owned(standing.as_ref(), &claims) else {
            return Ok(None);
        };
        let epoch = open_epoch(standing.as_ref());
        Ok(claims
            .into_iter()
            .find(|(claimant, claim)| *claimant == owner && claim.epoch == epoch)
            .map(|(_, claim)| claim.value))
    }

    /// Every account's claim to `key`, stale ones included, ascending by entry
    /// id: one trie-bucket read.
    ///
    /// # Errors
    /// Any storage error.
    pub fn claimants(&self, key: &K) -> Result<Vec<(AccountId, Claim<V>)>, StoreError> {
        self.claims.entries_at(key)
    }

    /// The names the calling account claims that no authority has settled.
    ///
    /// # Errors
    /// Any storage error.
    pub fn my_pending(&self) -> Result<Vec<K>, StoreError> {
        let me = current_account();
        let mut pending = Vec::new();
        for (key, _) in self.claims.my_entries()? {
            let unsettled = match self.status(&key)? {
                Status::Pending { mine, .. } => mine,
                Status::Contested { claimants } => claimants.contains(&me),
                Status::Free | Status::Owned { .. } | Status::Lost { .. } => false,
            };
            if unsettled {
                pending.push(key);
            }
        }
        Ok(pending)
    }

    /// Settles `key` as the authority: answers the owner's release with a
    /// vacancy, then grants the open epoch to [`RegistryAuthority::pick`]'s
    /// claimant, if any. Returns the owner the name then has.
    ///
    /// Idempotent: a name already settled is left alone, so a trigger fired
    /// twice, or on several TEEs, writes at most once per decision.
    ///
    /// # Errors
    /// `ActionNotAllowed` if the calling account may not write verdicts, or
    /// the registry has no authority; any storage error.
    pub fn resolve(&mut self, key: &K) -> Result<Option<AccountId>, StoreError> {
        self.check_authority()?;
        Ok(self.settle(key)?.0)
    }

    /// [`resolve`](Self::resolve)s every claimed name, until `budget` names
    /// have had a verdict written; returns how many did. The backstop for a
    /// missed trigger, run from a timer.
    ///
    /// Reads every claim in the registry.
    ///
    /// # Errors
    /// As for [`resolve`](Self::resolve).
    pub fn resolve_all_pending(&mut self, budget: usize) -> Result<usize, StoreError> {
        self.check_authority()?;
        let mut names = BTreeMap::new();
        for (_, key, _) in self.claims.entries_with_owners()? {
            let _existing = names.entry(name_of(key.as_ref())).or_insert(key);
        }
        let mut settled = 0;
        for key in names.into_values() {
            if settled == budget {
                break;
            }
            if self.settle(&key)?.1 {
                settled += 1;
            }
        }
        Ok(settled)
    }

    fn check_authority(&self) -> Result<(), StoreError> {
        if A::TRIGGER == Trigger::Never {
            return Err(not_allowed(
                "Registry::resolve: this registry has no authority",
            ));
        }
        if !self.verdicts.may_write() {
            return Err(not_allowed(
                "Registry::resolve: only the registry's authority writes verdicts",
            ));
        }
        Ok(())
    }

    /// Writes whatever verdicts `key` needs; returns its owner afterwards and
    /// whether anything was written.
    fn settle(&mut self, key: &K) -> Result<(Option<AccountId>, bool), StoreError> {
        let name = name_of(key.as_ref());
        let mut wrote = false;
        // Each pass writes a verdict at a higher epoch or returns, so this
        // ends: at most a vacancy, then a grant.
        loop {
            let claims = self.claims.entries_at(key)?;
            let standing = self.standing(key)?;
            if let Some(granted) = standing.as_ref().filter(|s| s.verdict.owner.is_some()) {
                let epoch = granted.verdict.epoch;
                let held = claims.iter().find(|(claimant, claim)| {
                    Some(*claimant) == granted.verdict.owner && claim.epoch == epoch
                });
                match held {
                    Some((owner, claim)) if claim.release => {
                        self.record(&name, Verdict::vacancy(&name, epoch, owner)?)?;
                        wrote = true;
                        continue;
                    }
                    Some((owner, _)) => return Ok((Some(*owner), wrote)),
                    // The granted claim has not reached this node: nothing to
                    // decide until it does.
                    None => return Ok((None, wrote)),
                }
            }

            let epoch = open_epoch(standing.as_ref());
            let candidates: Vec<_> = live(&claims, epoch)
                .map(|(owner, claim)| Candidate {
                    owner: *owner,
                    claim,
                    order: order(&claim_ref(&name, epoch, owner), false),
                })
                .collect();
            let Some(winner) = A::pick(&candidates).and_then(|at| candidates.get(at)) else {
                return Ok((None, wrote));
            };
            let owner = winner.owner;
            self.record(&name, Verdict::grant(&name, epoch, owner))?;
            return Ok((Some(owner), true));
        }
    }

    fn record(&mut self, name: &[u8; 32], verdict: Verdict) -> Result<(), StoreError> {
        let _previous = self.verdicts.write()?.insert(verdict.key(name), verdict)?;
        Ok(())
    }

    /// The verdict standing for `key`, by [`RegistryAuthority::standing`] over
    /// every well-formed verdict under its prefix. Reads every verdict of the
    /// name: one per decision and writing device.
    fn standing(&self, key: &K) -> Result<Option<Standing>, StoreError> {
        let Some(verdicts) = self.verdicts.read()? else {
            return Ok(None);
        };
        let name = name_of(key.as_ref());
        let ranked: Vec<Verdict> = verdicts
            .prefix(&name)?
            .filter(|(at, verdict)| *at == verdict.key(&name) && verdict.is_well_formed(&name))
            .map(|(_, verdict)| verdict)
            .collect();
        Ok(A::standing(&ranked)
            .and_then(|at| ranked.get(at))
            .map(|verdict| Standing {
                verdict: *verdict,
                stable: A::stable(verdict, &ranked),
            }))
    }

    /// The granted owner and its claim, when the standing verdict is a grant
    /// whose claim is here.
    fn owned<'c>(
        standing: Option<&Standing>,
        claims: &'c [(AccountId, Claim<V>)],
    ) -> Option<(AccountId, &'c Claim<V>)> {
        let standing = standing?;
        let owner = standing.verdict.owner?;
        claims
            .iter()
            .find(|(claimant, claim)| *claimant == owner && claim.epoch == standing.verdict.epoch)
            .map(|(owner, claim)| (*owner, claim))
    }
}

impl<K, V> Registry<K, V, Admin>
where
    K: StorageKey,
    V: BorshSerialize + BorshDeserialize + 'static,
{
    /// The accounts that may write verdicts.
    #[must_use]
    pub fn admins(&self) -> BTreeSet<AccountId> {
        self.verdicts.writers()
    }

    /// Replaces the admins. Only an admin may, and every node verifies it as a
    /// writer-set rotation: a verdict is checked against the admins as of that
    /// verdict.
    ///
    /// # Errors
    /// `ActionNotAllowed` if the caller is not an admin or the set is empty;
    /// any storage error.
    pub fn set_admins(&mut self, admins: BTreeSet<AccountId>) -> Result<(), StoreError> {
        self.verdicts.rotate_writers(admins)
    }
}

impl<K, V, A> Default for Registry<K, V, A>
where
    K: StorageKey,
    V: BorshSerialize + BorshDeserialize + 'static,
    A: RegistryAuthority,
{
    fn default() -> Self {
        Self::new()
    }
}

#[diagnostic::do_not_recommend]
impl<K, V, A> RekeyTarget for Registry<K, V, A>
where
    K: StorageKey,
    V: BorshSerialize + BorshDeserialize + 'static,
    A: RegistryAuthority,
{
    fn rekey_relative_to(&mut self, parent_id: Id) {
        self.claims
            .rekey_relative_to(field_child_id(parent_id, "claims"));
        self.verdicts
            .rekey_relative_to(field_child_id(parent_id, "verdicts"));
    }
}

/// Deliberately a no-op, as for [`Guarded`](super::Guarded): every claim and
/// verdict arrives as its own signed entry, checked against its stamp on apply.
#[diagnostic::do_not_recommend]
impl<K, V, A> Mergeable for Registry<K, V, A>
where
    K: StorageKey,
    V: BorshSerialize + BorshDeserialize + 'static,
    A: RegistryAuthority,
{
    fn merge(&mut self, _other: &Self) -> Result<(), MergeError> {
        Ok(())
    }
}

/// Structural: its entries merge by their own `crdt_type`, so there is no app
/// rule to dispatch.
#[diagnostic::do_not_recommend]
impl<K, V, A: RegistryAuthority> MergeStrategy for Registry<K, V, A> {
    const DISPATCHED: bool = false;
}

#[cfg(test)]
mod tests;
