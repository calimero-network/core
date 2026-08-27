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
//! **device signing key** as the device's `sign_pk` — because that is the key that
//! actually signs ops on the governance path. Crossing them is silent: a certificate
//! signed by the signing key itself still serializes, and is refused by every peer
//! while the local enrolment looks fine.
//!
//! That key is **one per node, not one per namespace**, even though the parameter
//! reaching here is still called a namespace identity — the name predates the
//! collapse of per-namespace identity. It matters more than naming: a reader who
//! takes it literally concludes a device needs re-certifying for every namespace it
//! joins, and so that an offline account root must come out of cold storage on every
//! join. One certificate covers the node everywhere.

use calimero_account::DeviceCert;
use calimero_context_client::local_governance::JoinAccountCredential;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::NodeDeviceRepository;
use calimero_primitives::identity::PublicKey;
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
/// Takes only the signing key's PUBLIC half: the certificate is signed by the
/// account root, and the namespace identity appears in it as a subject, never as a
/// signer. A joiner that had to hand its private key to this function would be
/// handing it over for nothing.
///
/// # Errors
/// Propagates the store read/write, the account-root generation, or a signing
/// failure. A joiner that cannot build this cannot join — deliberately: joining
/// without it leaves the member account-addressed later than it is
/// member-addressed, and a grant made in that window names a principal the
/// member's own writes do not present.
pub fn build(
    datastore: &Store,
    namespace_id: &ContextGroupId,
    signing_pk: &PublicKey,
) -> EyreResult<Box<JoinAccountCredential>> {
    let devices = NodeDeviceRepository::new(datastore);
    // The existing row, READ not minted. Minting is what needs a root, and the
    // ordering is forced: a certificate is signed over a device id, a signing key
    // and an agreement key, so the device has to exist before anybody can certify
    // it. An import therefore always finds a row already there.
    let existing = devices
        .get()
        .wrap_err("join credential: could not read this node's device row")?;

    // An imported certificate wins, and is the only path open to a root-free node.
    //
    // Checked first rather than as a fallback: importing one is a deliberate
    // operator act, so a node that holds both a root and an import must present the
    // import — silently preferring the self-signed one would ignore the operator
    // and, worse, would speak for a different account than the one the import
    // named.
    if let Some(stored) = devices
        .imported_certificate()
        .wrap_err("join credential: could not read the imported certificate")?
    {
        let proof: JoinAccountCredential = borsh::from_slice(&stored).wrap_err(
            "join credential: the stored certificate could not be decoded; re-import it \
             with `merod account import-cert`",
        )?;
        let enrolled = existing.ok_or_else(|| {
            eyre::eyre!(
                "join credential: a certificate is imported but this node has no device \
                 row, so there is nothing it can describe — the device is minted first \
                 and certified second"
            )
        })?;
        certificate_matches_this_node(&proof, &enrolled, signing_pk)?;
        return Ok(Box::new(proof));
    }

    let root = devices
        .require_account_root()
        .wrap_err("join credential: could not resolve this node's account root")?;
    let enrolled = devices
        .ensure_enrolled(namespace_id)
        .wrap_err("join credential: could not mint this node's device")?;

    let cert = DeviceCert::sign(
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
        // Both epochs above are 0 and the chain is empty, and that is a guarantee
        // rather than an assumption: nothing in the tree rotates a node's OWN
        // account root. `RootKeyHandoff` is produced by no node-side flow, so a
        // root this node minted has never advanced past epoch 0 and there are no
        // handoffs for a verifier to walk. `ensure_enrolled` likewise returns the
        // same device on a rejoin rather than bumping its epoch, and re-presenting
        // an identical credential is admitted rather than refused as stale.
        //
        // When rotation does arrive, both epochs have to be read from the group's
        // own binding rows (`account_key` for the key epoch, `raw_binding` for the
        // device epoch) — a first join has no such rows to read, so that is a
        // different function, not a bigger one.
        chain: vec![],
        statement: cert,
    }))
}

/// Refuse an imported certificate that does not describe THIS node.
///
/// Every field here is one a peer checks and this node cannot see the result of: a
/// certificate naming another device, or a signing key this node does not author
/// with, produces a join that verifies as a *credential* and is then refused as a
/// binding. The join fails at the peer with nothing local to point at, which is the
/// worst place to discover a mistyped `--device`.
///
/// # Errors
/// If the certificate's account, device, signing key or agreement key is not this
/// node's.
fn certificate_matches_this_node(
    proof: &JoinAccountCredential,
    enrolled: &calimero_governance_store::NodeDevice,
    signing_pk: &PublicKey,
) -> EyreResult<()> {
    let cert = &proof.statement;

    // The account first: the device row already names an account, and if the
    // certificate names another then `account_for_group` and this credential
    // disagree about who the node is — one resolver answering differently from the
    // other is the failure the account plane is least able to diagnose.
    eyre::ensure!(
        cert.account == enrolled.account,
        "the imported certificate is for account {} but this node's device belongs to {}; \
         import the certificate signed for THIS node's account",
        cert.account,
        enrolled.account,
    );
    eyre::ensure!(
        cert.device == enrolled.device(),
        "the imported certificate is for device {} but this node is {}",
        cert.device,
        enrolled.device(),
    );
    eyre::ensure!(
        &cert.sign_pk == signing_pk,
        "the imported certificate certifies a signing key this node does not author with; \
         it was signed over another node's key",
    );
    eyre::ensure!(
        cert.kem_pk == enrolled.kem_public_key(),
        "the imported certificate names an agreement key that is not this device's, so \
         scope keys wrapped to it could not be opened here",
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_account::{AccountGenesis, DeviceCert};
    use calimero_governance_store::NodeDeviceRepository;
    use calimero_primitives::identity::PrivateKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;

    use super::{build, ContextGroupId, JoinAccountCredential, PublicKey};

    /// A root-free node: no account root, but a device row adopted under an
    /// account whose root lives on another machine.
    ///
    /// This is the whole posture under test. The root exists only as a local
    /// variable here, standing in for the machine that holds it — the store never
    /// sees it, which is what the assertions below rely on.
    fn root_free_node(root_sk: &PrivateKey) -> (Store, ContextGroupId) {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ns = ContextGroupId::from([9u8; 32]);
        let genesis = AccountGenesis::new(root_sk.public_key());
        NodeDeviceRepository::new(&store)
            .ensure_enrolled_into(&ns, genesis)
            .expect("adopt an account rooted elsewhere");
        (store, ns)
    }

    /// Certify whatever the caller names, as the offline holder would.
    fn certify(
        root_sk: &PrivateKey,
        store: &Store,
        device_override: Option<calimero_primitives::identity::DeviceId>,
        sign_pk: &PublicKey,
    ) -> Vec<u8> {
        let held = NodeDeviceRepository::new(store)
            .get()
            .expect("read")
            .expect("device row");
        let cert = DeviceCert::sign(
            root_sk,
            held.account,
            device_override.unwrap_or_else(|| held.device()),
            sign_pk,
            &held.kem_public_key(),
            0,
            0,
        )
        .expect("sign");
        borsh::to_vec(&JoinAccountCredential {
            genesis: held.genesis,
            chain: vec![],
            statement: cert,
        })
        .expect("encode")
    }

    #[test]
    fn a_root_free_node_presents_its_imported_certificate() {
        let root_sk = PrivateKey::from([3u8; 32]);
        let (store, ns) = root_free_node(&root_sk);
        let signing_pk = PrivateKey::from([4u8; 32]).public_key();
        let repo = NodeDeviceRepository::new(&store);

        // Without one, there is nothing to present and no root to sign with.
        let err = build(&store, &ns, &signing_pk)
            .expect_err("a root-free node with no certificate cannot build a credential");
        assert!(err.to_string().contains("account root"), "{err}");

        repo.store_imported_certificate(&certify(&root_sk, &store, None, &signing_pk))
            .expect("import");

        let credential = build(&store, &ns, &signing_pk).expect("present the imported cert");
        assert_eq!(
            credential.statement.sign_pk, signing_pk,
            "the presented certificate must be the imported one",
        );
        assert!(
            repo.account_root().expect("read").is_none(),
            "and presenting it must not have minted a root — that is the whole point",
        );
    }

    /// A certificate for another device must be refused HERE, not by a peer.
    #[test]
    fn a_certificate_naming_another_device_is_refused() {
        let root_sk = PrivateKey::from([3u8; 32]);
        let (store, ns) = root_free_node(&root_sk);
        let signing_pk = PrivateKey::from([4u8; 32]).public_key();

        let other = calimero_primitives::identity::DeviceId::from([0xAB; 32]);
        NodeDeviceRepository::new(&store)
            .store_imported_certificate(&certify(&root_sk, &store, Some(other), &signing_pk))
            .expect("import");

        let err = build(&store, &ns, &signing_pk).expect_err("must not present another device's");
        assert!(err.to_string().contains("device"), "{err}");
    }

    /// The certified signing key must be the one this node actually authors with.
    ///
    /// Crossing these is the silent failure the module header warns about: the
    /// credential verifies as a credential and is refused as a binding, so the join
    /// dies at a peer with nothing local to point at.
    #[test]
    fn a_certificate_over_another_signing_key_is_refused() {
        let root_sk = PrivateKey::from([3u8; 32]);
        let (store, ns) = root_free_node(&root_sk);
        let mine = PrivateKey::from([4u8; 32]).public_key();
        let theirs = PrivateKey::from([5u8; 32]).public_key();

        NodeDeviceRepository::new(&store)
            .store_imported_certificate(&certify(&root_sk, &store, None, &theirs))
            .expect("import");

        let err = build(&store, &ns, &mine).expect_err("must not present a cert over another key");
        assert!(err.to_string().contains("signing key"), "{err}");
    }

    /// Garbage in the row must name itself, and say how to recover.
    #[test]
    fn an_undecodable_certificate_says_how_to_fix_it() {
        let root_sk = PrivateKey::from([3u8; 32]);
        let (store, ns) = root_free_node(&root_sk);

        NodeDeviceRepository::new(&store)
            .store_imported_certificate(&[0xFF; 32])
            .expect("store garbage");

        let err = build(&store, &ns, &PrivateKey::from([4u8; 32]).public_key())
            .expect_err("undecodable bytes cannot be presented");
        let msg = format!("{err:#}");
        assert!(msg.contains("import-cert"), "{msg}");
    }
}
