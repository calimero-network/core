//! **Account identity** — one identity, many devices.
//!
//! The unified causal log authenticates an op with the key that signed it and
//! authorizes it against a folded ACL. Historically those were the *same* key:
//! a per-namespace member key that simultaneously served as the signing key,
//! the ACL subject, the scope-key delivery recipient, and the CRDT replica id.
//! One key doing four jobs is why a person could not hold two devices — sharing
//! the key corrupts the CRDT planes (counter slots and HLC seeds are keyed by
//! replica id and assume one writer each), and *not* sharing it splits one
//! person into two unrelated members.
//!
//! This crate introduces the two ids that break the conflation:
//!
//! - [`AccountId`] — the person. The **only** authorization subject, and the
//!   app-visible "who wrote this". Never signs anything itself.
//! - [`DeviceId`] — one installation. The CRDT replica id, and the holder of
//!   the keypair that actually signs ops. **Never** an authorization input.
//!
//! # Ids are not keys
//!
//! The governing principle: *every identity is a stable id whose current key
//! set is projected state*. Both ids are content addresses, not public keys, so
//! every key in the system can rotate without the identity changing. Making
//! `AccountId` the account's public key — the obvious shortcut — would mean the
//! root key can never rotate (rotating would mean becoming a different person
//! and losing all membership and authorship), and it would force the root key
//! out of cold storage for routine work.
//!
//! # Self-certifying anchor
//!
//! [`AccountId`] is the content address of the account's [`AccountGenesis`],
//! and the genesis names the epoch-0 root key. So *the initial root key is
//! recoverable from the account id itself*: a verifier handed a
//! [`DeviceCert`] can check `AccountGenesis::account_id() == cert.account` and
//! then walk a signed [`RootKeyHandoff`] chain up to the cert's epoch, with **no
//! prior state and no ordering dependency**. That is what lets a device link
//! itself into a scope in a single self-contained op.
//!
//! What this crate does *not* do is decide anything. It is pure verification of
//! self-contained credentials; the at-cut checks that make a credential
//! *authoritative* (has this root key been superseded? has this device been
//! revoked? is the account even a member here?) belong to the projection and
//! `calimero-authz`, because only those see the causal cut.
//!
//! # Where things live
//!
//! | Module | What it holds |
//! | --- | --- |
//! | `signed` | The shape every root-signed credential shares: [`RootSigned`], [`Verified`], [`AccountProof`] |
//! | `account` | The account anchor: [`AccountGenesis`] and a member's [`AccountMemberEndorsement`] of it |
//! | `root_key` | Root-key rotation: [`RootKeyHandoff`] and the chain walk, [`root_key_at_epoch`] |
//! | `device` | Device credentials: [`KemPublicKey`], [`DeviceCert`], and its verification |
//! | `revocation` | Withdrawing a device: [`DeviceRevocation`] and its self-contained proof |
//! | `pairing` | Linking a new device: [`PairingOffer`], its statement, and the human-compared code |
//! | `domain` | Every signing domain in one place, so they stay pairwise distinct |
//! | `error` | [`AccountError`] — why a credential failed |
//!
//! Each module's header carries a `# Why it is shaped this way` section holding
//! the design arguments for what is in it, so an item's own docs can stay the
//! contract a caller needs: what it checks, what it returns, and how it fails.
//!
//! Every public item is re-exported here, so `calimero_account::DeviceCert`
//! keeps working regardless of which module it moved to.

mod account;
mod device;
mod domain;
mod error;
mod pairing;
mod revocation;
mod root_key;
mod signed;

#[cfg(test)]
mod tests;

// `AccountId` and `DeviceId` live in `calimero-primitives` beside `PublicKey`,
// where the rest of the system's shared ids live. They have to: they are named
// in client-facing event payloads that `calimero-primitives` defines, and this
// crate depends on that one, so the types cannot originate here. Re-exported so
// every `calimero_account::AccountId` import keeps working — this crate is
// still where the account MODEL lives, just not where the id type is declared.
pub use calimero_primitives::identity::{
    domain_hash, AccountId, DeviceId, IdParseError, DEVICE_ID_DOMAIN,
};

pub use crate::account::{
    AccountGenesis, AccountMemberEndorsement, VerifiedEndorsement, ACCOUNT_GENESIS_VERSION,
};
pub use crate::device::{DeviceCert, KemPublicKey, VerifiedDeviceCert};
pub use crate::error::AccountError;
pub use crate::pairing::PairingOffer;
pub use crate::revocation::{DeviceRevocation, SignedDeviceRevocation, VerifiedDeviceRevocation};
pub use crate::root_key::{root_key_at_epoch, RootKeyHandoff, MAX_ROOT_KEY_HANDOFFS};
pub use crate::signed::{AccountProof, RootSigned, Verified};

// The three end-to-end verifiers keep free-function form: each takes an anchor and
// a BORROWED chain, which is what the apply paths hold, and a method would force
// them to allocate an `AccountProof` per check just to throw it away. A caller
// that already has a proof should use `AccountProof::verify` instead.
pub use crate::device::verify_device_cert;
pub use crate::revocation::verify_device_revocation;
pub use crate::signed::verify_root_signed;
