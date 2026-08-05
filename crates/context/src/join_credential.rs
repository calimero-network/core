//! Builds the account credential a joiner carries on its join op.
//!
//! One helper, used by all three cleartext join paths (`join_group`,
//! `join_context`'s open self-join, and `join_subgroup_inheritance`), because the
//! credential must be identical in shape wherever a join is published — a joiner
//! that assembled it differently per path would be a joiner whose account depended
//! on how it got in.
//!
//! # Why this can be built before the node holds a scope key
//!
//! Nothing here publishes or encrypts. `ensure_enrolled` mints the node's device row
//! **locally** and returns the genesis it derives from the account root; the
//! certificate is signed by that root. So the whole credential is available at join
//! time, which is exactly what makes carrying it on a cleartext join op possible —
//! and why enrolment no longer has to wait for key delivery.
//!
//! # The two keys, one use each
//!
//! The certificate is signed by the **account root**, and records the node's
//! **namespace identity** as the device's `sign_pk` — because that is the key that
//! actually signs ops on the governance path. Crossing them is silent: a certificate
//! signed by the namespace identity still serializes, and is refused by every peer
//! while the local enrolment looks fine.

use calimero_account::sign_device_cert;
use calimero_context_client::local_governance::JoinAccountCredential;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::NodeDeviceRepository;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::Store;
use eyre::{Result as EyreResult, WrapErr as _};

/// Mint (or recover) this node's device for `namespace` and certify it.
///
/// Returns the credential **boxed**, matching the op field: the credential is ~253
/// bytes and inlining it in three `RootOp` variants pushed the enum past clippy's
/// `large_enum_variant` threshold. Borsh encodes `Box<T>` as `T`, so this is
/// invisible on the wire. Boxing here rather than at each call site keeps all four
/// publishers identical.
///
/// Idempotent: `ensure_enrolled` returns the existing device when there is one, so a
/// rejoin carries the same credential rather than minting a second replica id and
/// stranding the CRDT state held under the first.
///
/// # Errors
/// Propagates the store read/write, the account-root generation, or a signing
/// failure. A joiner that cannot build this cannot join — deliberately: joining
/// without it is the pre-#3346 state where the member is account-addressed later
/// than it is member-addressed, which is the window #3378 fell into.
pub fn build(
    datastore: &Store,
    namespace_id: &ContextGroupId,
    signing_pk: &PublicKey,
    _signing_sk: &PrivateKey,
) -> EyreResult<Box<JoinAccountCredential>> {
    let devices = NodeDeviceRepository::new(datastore);
    let root = devices
        .ensure_account_root()
        .wrap_err("join credential: could not resolve this node's account root")?;
    let enrolled = devices
        .ensure_enrolled(namespace_id)
        .wrap_err("join credential: could not mint this node's device")?;

    let cert = sign_device_cert(
        root.signing_key(),
        enrolled.account,
        enrolled.device(),
        signing_pk,
        &enrolled.kem_public_key(),
        0,
        0,
    )
    .map_err(|err| eyre::eyre!("join credential: failed to sign the device cert: {err}"))?;

    Ok(Box::new(JoinAccountCredential {
        genesis: enrolled.genesis,
        // Epoch 0 with an empty chain: the account root has not rotated, so there
        // are no handoffs for a verifier to walk. Same shape as every other
        // credential this node mints today.
        chain: vec![],
        cert,
    }))
}
