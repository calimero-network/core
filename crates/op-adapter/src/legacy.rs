//! The stand-in account derivation, and the two rules built on it.
//!
//! Per-plane ops name a bare signing key because they predate accounts; the
//! unified planes are keyed by [`AccountId`]. Everything here is that seam, and
//! nothing here outlives it — the crate is deleted at cutover.

use calimero_account::{AccountId, DeviceId};
use calimero_op::Authorship;
use calimero_primitives::identity::PublicKey;
use sha2::{Digest, Sha256};

/// Domain separator for the adapter's stand-in account derivation. Distinct
/// from every domain in `calimero-account` so a value minted here can never be
/// mistaken for — or collide with — a real account id.
const LEGACY_ACCOUNT_DOMAIN: &[u8] = b"calimero.op-adapter.legacy-account.v1";

/// The stand-in account for a legacy member key.
///
/// **Adapter-local, and not a protocol concept.** Per-plane ops name a bare
/// member key because they predate accounts; the unified planes are keyed by
/// [`AccountId`]. This derivation is the seam between the two, and it exists
/// only so the fold-equivalence proofs can compare like with like.
///
/// It is deliberately *not* something `calimero-account` offers. A real account
/// is the content address of a genesis whose root key can rotate and whose
/// devices can be revoked; a value derived from a bare key has none of those
/// properties, and exposing one as a first-class account would quietly
/// reintroduce the id-equals-key conflation the account model exists to remove.
/// Living in the adapter — the crate that is deleted at cutover — is what keeps
/// it from outliving the transition.
#[must_use]
pub fn legacy_account_id(member: &PublicKey) -> AccountId {
    let mut hasher = Sha256::new();
    hasher.update((LEGACY_ACCOUNT_DOMAIN.len() as u64).to_le_bytes());
    hasher.update(LEGACY_ACCOUNT_DOMAIN);
    hasher.update(AsRef::<[u8; 32]>::as_ref(member));
    let digest: [u8; 32] = hasher.finalize().into();
    AccountId::from(digest)
}

/// The account a signing key writes as, on the **writer** plane.
///
/// One rule, used at both ends: the node deciding what to put in a writer set
/// (`env::account_id()`), and the peer resolving an incoming signature against
/// one. They have to agree, and they can only agree by sharing this.
///
/// `binding` is that key's device binding — its real [`AccountId`] — if one has
/// been published. Absent that, the key writes as its own stand-in, which is what
/// any peer can derive from the key alone. That fallback is what makes an
/// unenrolled node usable at all: it has an account nobody else can compute (an
/// id derived from its private root), so naming *that* in a writer set would
/// produce a grant no peer could ever match.
///
/// **The precedence is the opposite of the membership plane's, deliberately.**
/// There, a key that is a member in its own right *is* that member and a binding
/// is only a fallthrough — preferring the binding erased members whose rows are
/// keyed by the stand-in. Here the writer set is populated from
/// `env::account_id()`, so resolution has to answer in whatever space that
/// returns, and the binding is what makes a person's second device write under
/// the same principal as their first. The two planes converge when the legacy
/// bridge retires.
///
/// Consequence worth stating: a writer set seeded BEFORE the writer enrolled
/// holds the stand-in, so enrolling afterwards changes what that key writes as and
/// the old grant goes stale. Re-grant after `account create`.
#[must_use]
pub fn writer_account(binding: Option<AccountId>, key: &PublicKey) -> AccountId {
    binding.unwrap_or_else(|| legacy_account_id(key))
}

/// The [`Authorship`] a bridged legacy op carries.
///
/// Legacy ops name only a signing key, so the device is derived from the
/// stand-in account rather than enrolled. Like [`legacy_account_id`], this is a
/// transition artifact: natively authored ops carry a real enrolled device.
#[must_use]
pub fn legacy_authorship(signer: PublicKey) -> Authorship {
    let account = legacy_account_id(&signer);
    Authorship {
        account,
        device: DeviceId::from(*account.as_bytes()),
        device_key: signer,
    }
}
