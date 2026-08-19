//! Who authored an op — the three ids that one key used to answer alone.
//!
//! Kept apart from the envelope in [`op`](crate::op) because the split is a
//! model decision, not an envelope detail: the account authorizes, the device
//! is the CRDT replica, the key signs. [`Authorship`] is what forces all three
//! to travel together, and its sentinel is what makes "we could not attribute
//! this" a value the gates already refuse rather than a case each one has to
//! remember.

use borsh::{BorshDeserialize, BorshSerialize};

use calimero_account::{AccountId, DeviceId};
use calimero_primitives::identity::PublicKey;

/// Who authored an op, as one indivisible triple.
///
/// These three answer three *different* questions that used to be answered by a
/// single key, and separating them is what makes one identity across several
/// devices possible:
///
/// - [`account`](Self::account) — **who**, for authorization and for the app.
///   The only subject the ACL and membership planes key on.
/// - [`device`](Self::device) — **which replica**, for the CRDT planes. Must be
///   unique per concurrent writer; never an authorization input.
/// - [`device_key`](Self::device_key) — **what signed this**, for integrity.
///
/// They travel together because a claim is only meaningful as a unit: the
/// signature proves the device key authored the op, and the projection proves
/// that key currently speaks for that account. Splitting them across call
/// boundaries invites checking one without the other.
///
/// All three are covered by [`Op::compute_id`](crate::Op::compute_id), hence by
/// the signature. If `device_key` were left out, an attacker could swap in their
/// own key; if `account` were left out, a device's op could be replayed under a
/// different account.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, BorshSerialize, BorshDeserialize)]
pub struct Authorship {
    /// The authorizing identity — what ACLs, membership, and the app see.
    ///
    /// **Carried on the op, and it has to be.** The obvious simplification is to
    /// drop this and resolve the author from the folded device bindings instead —
    /// the membership plane already does exactly that, in
    /// `ScopeProjections::account_for_author`. It does not work here, and the
    /// reason is recorded in `calimero-projection`'s fold: a projection folds
    /// **raw logs** (`from_ops` and the sync convergence path both do), so a
    /// binding read mid-fold answers "has that device's link folded *yet*"
    /// rather than "what does this key speak for". A revoked device once stored
    /// either the binding's account or the payload's claim depending on which
    /// arrived first, and that value is hashed into `governance_hash` — so it
    /// **split the root by arrival order**.
    ///
    /// The membership plane escapes this because it resolves against a *fixed
    /// cut*, where the answer is a function of the op set rather than its order.
    /// The fold has no cut to resolve against; it is what builds one.
    ///
    /// So a producer establishes this before the op is folded — from a credential
    /// the op carries, or from a binding it resolved while applying — and an op
    /// nothing can attribute gets [`Self::UNATTRIBUTED_ACCOUNT`] rather than an
    /// invented account or a `None` every consumer must remember to handle.
    pub account: AccountId,
    /// The CRDT replica id of the installation that authored this.
    pub device: DeviceId,
    /// The Ed25519 key that produced [`Op::signature`](crate::Op::signature).
    pub device_key: PublicKey,
}

impl Authorship {
    /// The account id that names **nobody** — the answer for an op whose author
    /// cannot be established.
    ///
    /// A key becomes attributable through a credential the op carries or a
    /// binding a producer resolved. When neither exists there is no principal to
    /// name, and the honest record of that is one well-known value rather than a
    /// per-key derivation. The old stand-in hashed the signing key into an
    /// account-shaped id, which read as a real principal at every call site that
    /// saw it, and *looked* different for every key — so "we could not attribute
    /// this" was indistinguishable from "this is somebody".
    ///
    /// Every gate on this plane asks whether the author equals some real
    /// principal, so a value no genesis can produce **fails closed everywhere by
    /// construction**: membership, admin, ownership and the account-plane
    /// handoff check all answer "no" without any of them needing a special case.
    /// That is why this is a sentinel and not an `Option` — an option would put
    /// the same decision in a dozen places and rely on each getting it right.
    /// See [`Authorship::account`] for why the field is carried at all rather
    /// than resolved during the fold.
    pub const UNATTRIBUTED_ACCOUNT: AccountId = AccountId::from_raw([0u8; 32]);

    /// The device id paired with [`Self::UNATTRIBUTED_ACCOUNT`].
    pub const UNATTRIBUTED_DEVICE: DeviceId = DeviceId::from_raw([0u8; 32]);

    /// Authorship for an op whose author could not be established.
    ///
    /// `device_key` is still recorded, because it is a fact: that key signed the
    /// op, and `Op::verify` checks the signature against it. Only the principal
    /// it speaks for is unknown.
    #[must_use]
    pub const fn unattributed(device_key: PublicKey) -> Self {
        Self {
            account: Self::UNATTRIBUTED_ACCOUNT,
            device: Self::UNATTRIBUTED_DEVICE,
            device_key,
        }
    }
}
