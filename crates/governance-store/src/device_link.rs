//! Extending a device this account already certified into one more namespace.
//!
//! Pairing binds a device wherever the holder took part *at that moment*, and
//! nothing repeated it afterwards - so a namespace created or joined later had no
//! binding for a device paired before it, and the paired device silently never saw
//! it. This is the repeat.
//!
//! It is cheap because a [`DeviceCert`](calimero_account::DeviceCert) carries no
//! namespace and no expiry. Binding a known device somewhere new therefore needs
//! only the stored certificate, a fresh endorsement any member of that namespace
//! can sign, and one key wrap. No handshake, no confirmation code, and the device
//! need not be online.
//!
//! One publish path, used by all three callers - pairing's fan-out, the auto-bind
//! that runs when this node gains a namespace, and the relink endpoint that
//! repairs drift.

use std::sync::Arc;

use calimero_account::{AccountMemberEndorsement, AccountProof, DeviceCert, DeviceId};
use calimero_context_client::local_governance::{AckRouter, GroupOp, NamespaceOp, RootOp};
use calimero_context_config::types::ContextGroupId;
use calimero_crypto::X25519PublicKey;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::identity::PrivateKey;
use calimero_store::Store;
use eyre::Result as EyreResult;
use tracing::{info, warn};

use crate::{
    AccountBindingRepository, GroupKeyring, KnownDeviceCert, MetaRepository, NodeDeviceRepository,
};

/// What binding one device into one namespace came to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum BindOutcome {
    /// The link was published. `key_delivered` false means the link landed and
    /// the scope-key delivery did not - the link is what confers authority, and
    /// the device's own sync pull is the durable retry for the key.
    Linked { key_delivered: bool },
    /// The device's scope does not reach the application this namespace serves.
    OutOfScope,
    /// A tombstone here. Revocation is terminal, so the id can never be linked
    /// again - in this account or any other.
    Revoked,
    /// A live binding already, so there is nothing to repair.
    AlreadyBound,
    /// This node holds no current scope key here, so it can neither publish an
    /// encrypted group op nor deliver one.
    NoScopeKey,
    /// This node's own device, which ordinary enrolment already covers.
    OwnDevice,
    /// Nothing was published. The reason is logged where it is known; a namespace
    /// gain must not fail because a device could not be extended into it.
    Failed,
}

/// Publish, or the reason not to.
enum BindPlan {
    /// Go ahead, under this scope key.
    Publish([u8; 32]),
    Skip(BindOutcome),
}

/// Everything that decides whether the pair of publishes is worth making,
/// cheapest question first.
fn plan(store: &Store, namespace: &ContextGroupId, cert: &KnownDeviceCert) -> EyreResult<BindPlan> {
    let device = cert.device();
    let devices = NodeDeviceRepository::new(store);

    if devices.get()?.is_some_and(|held| held.device() == device) {
        return Ok(BindPlan::Skip(BindOutcome::OwnDevice));
    }
    // The namespace's target application, read the way the pairing fan-out reads
    // it. A namespace whose metadata has not synced names none, and is reachable
    // only by a scope that names none either.
    let application = MetaRepository::new(store)
        .load(namespace)?
        .map(|meta| meta.target_application_id);
    if !cert.covers(application) {
        return Ok(BindPlan::Skip(BindOutcome::OutOfScope));
    }

    let bindings = AccountBindingRepository::new(store);
    if bindings.is_revoked(namespace, device)? {
        return Ok(BindPlan::Skip(BindOutcome::Revoked));
    }
    if bindings.is_device_linked(namespace, device)? {
        return Ok(BindPlan::Skip(BindOutcome::AlreadyBound));
    }

    let Some((_key_id, ns_key)) = GroupKeyring::new(store, *namespace).load_current_key()? else {
        return Ok(BindPlan::Skip(BindOutcome::NoScopeKey));
    };
    Ok(BindPlan::Publish(ns_key))
}

/// Publish the two ops that make a device usable in `namespace`.
///
/// The link confers authority; the delivery hands over the key without which the
/// link lands and the device still cannot read anything. Both are load-bearing,
/// which is why they live in one function rather than at three call sites.
///
/// The delivery has to be a cleartext `RootOp`: a device-addressed envelope
/// carried inside an *encrypted* group op would be unreadable by its only
/// recipient.
///
/// **The endorsement is minted here, from the key that also signs the ops.** The
/// two keys in play - the account root that certified the device and the
/// namespace identity that endorses it - are trivially crossed, and a crossed
/// pair produces a link every peer refuses while looking perfectly healthy
/// locally. Taking one signing key makes that unrepresentable rather than
/// merely checked.
///
/// `Ok(false)` means the link landed and the delivery did not - not a failed
/// bind. An `Err` means nothing was published: the wrap runs first precisely so a
/// device this node cannot address never gets a link published for it.
///
/// # Errors
/// If the endorsement cannot be signed, the scope key cannot be wrapped for the
/// device, or the link fails to publish.
pub async fn publish_link_and_key(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &Arc<AckRouter>,
    namespace: &ContextGroupId,
    signer_sk: &PrivateKey,
    proof: &AccountProof<DeviceCert>,
    ns_key: &[u8; 32],
) -> EyreResult<bool> {
    let cert = &proof.statement;
    let device = cert.device;

    // Wrapped under the KEM key the certificate names rather than one read off a
    // folded binding, so the delivery does not depend on this node having already
    // folded the link it is about to publish.
    let envelope = GroupKeyring::wrap_for_device(
        signer_sk,
        device,
        &X25519PublicKey::from(*cert.kem_pk.as_bytes()),
        &namespace.to_bytes(),
        ns_key,
    )?;

    // Only a member can endorse and only the root can certify; the apply gate
    // needs both, and a fresh endorsement is all a namespace gained later is
    // missing.
    let endorsement = AccountMemberEndorsement::sign(signer_sk, cert.account)
        .map_err(|err| eyre::eyre!("failed to endorse device {device}: {err}"))?;
    let link = GroupOp::AccountDeviceLinked {
        genesis: proof.genesis,
        chain: proof.chain.clone(),
        cert: *cert,
        endorsement,
    };

    let report =
        crate::sign_apply_and_publish(store, node_client, ack_router, namespace, signer_sk, link)
            .await?;
    info!(
        namespace_id = ?namespace,
        %device,
        published = report.is_some(),
        "linked a device of this account"
    );

    // `required_signers` is None because the device is not a member and so is not
    // among the acking set - its receipt shows up as the device being able to
    // read, not as an ack.
    let delivery = NamespaceOp::Root(RootOp::KeyDelivery {
        group_id: namespace.to_bytes().into(),
        envelope,
    });
    if let Err(err) = crate::sign_and_publish_namespace_op(
        store,
        node_client,
        ack_router,
        namespace.to_bytes().into(),
        signer_sk,
        delivery,
        None,
    )
    .await
    {
        warn!(
            ?err,
            namespace_id = ?namespace,
            %device,
            "device linked but the scope-key delivery failed to publish; \
             the device's sync pull is the durable retry"
        );
        return Ok(false);
    }
    Ok(true)
}

/// Bind `cert`'s device into `namespace` unless something makes it pointless or
/// forbidden.
///
/// Never fails: every outcome is a value. A namespace gain is the primary
/// operation and extending a device into it is best-effort on top, so a caller
/// must not be able to propagate a failure here by accident.
pub async fn ensure_bound(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &Arc<AckRouter>,
    namespace: &ContextGroupId,
    signer_sk: &PrivateKey,
    cert: &KnownDeviceCert,
) -> BindOutcome {
    let device = cert.device();
    let ns_key = match plan(store, namespace, cert) {
        Ok(BindPlan::Publish(ns_key)) => ns_key,
        Ok(BindPlan::Skip(outcome)) => return outcome,
        Err(err) => {
            warn!(namespace_id = ?namespace, %device, %err,
                  "could not decide whether to bind this device here; leaving it unbound");
            return BindOutcome::Failed;
        }
    };

    match publish_link_and_key(
        store,
        node_client,
        ack_router,
        namespace,
        signer_sk,
        &cert.proof,
        &ns_key,
    )
    .await
    {
        Ok(key_delivered) => BindOutcome::Linked { key_delivered },
        Err(err) => {
            warn!(namespace_id = ?namespace, %device, %err,
                  "could not extend a known device into this namespace");
            BindOutcome::Failed
        }
    }
}

/// Extend every device this account has already certified into `namespace`.
///
/// Runs when this node gains a namespace and holds its scope key. Best-effort by
/// construction: it returns what happened rather than a `Result`, so a join or a
/// creation cannot fail because one device could not be carried across.
pub async fn bind_known_devices(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &Arc<AckRouter>,
    namespace: &ContextGroupId,
    signer_sk: &PrivateKey,
) -> Vec<(DeviceId, BindOutcome)> {
    let certs = match NodeDeviceRepository::new(store).device_certs() {
        Ok(certs) => certs,
        Err(err) => {
            warn!(namespace_id = ?namespace, %err,
                  "could not read this account's device certificates; binding none here");
            return Vec::new();
        }
    };

    let mut outcomes = Vec::with_capacity(certs.len());
    for cert in &certs {
        let outcome =
            ensure_bound(store, node_client, ack_router, namespace, signer_sk, cert).await;
        outcomes.push((cert.device(), outcome));
    }
    outcomes
}
