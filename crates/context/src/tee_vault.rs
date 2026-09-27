//! Hands the namespace TEE key to every TEE authority.
//!
//! A value only the TEE may read is sealed to one namespace key
//! (`calimero_governance_store::tee_vault_keys`), so that a TEE admitted after
//! it was sealed can still open it. This task is what gets the key to that TEE:
//! on a node that is a TEE authority, it publishes a
//! `GroupOp::TeeVaultKeyDelivered` of every key it holds for every TEE
//! authority without a copy, and creates the key when the namespace has none.
//!
//! It sweeps on a timer rather than reacting to events: a TEE becomes an
//! authority through several ops (admission, evidence, the authoring policy)
//! that land in any order and on any node, and a sweep needs no event from any
//! of them. On a node that is not a TEE member of a namespace it costs two
//! point lookups there.
//!
//! A key reaches a new TEE only while some TEE that holds it is up. That is no
//! worse than sealing to each TEE's own key, and after one delivery the new TEE
//! can hand the key on itself.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use calimero_context_client::local_governance::{AckRouter, GroupOp};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::governance_broadcast::ObserveDelivery;
use calimero_governance_store::{
    seal_tee_vault_key, sign_apply_and_publish, tee_authority_keys_in_namespace,
    tee_vault_deliveries, tee_vault_keys, MembershipRepository, NamespaceRepository,
    TeeVaultDelivery,
};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::Store;
use eyre::{eyre, Result as EyreResult};
use tokio::task::AbortHandle;
use tracing::{debug, info, warn};

/// How often each namespace is checked.
const SWEEP: Duration = Duration::from_secs(10);

static HANDLE: Mutex<Option<AbortHandle>> = Mutex::new(None);

/// Start the task for the life of the node. A no-op while one is running;
/// [`shutdown`] first to rebind it to a new store or client.
pub(crate) fn spawn(store: Store, node_client: NodeClient, ack_router: Arc<AckRouter>) {
    let Ok(mut slot) = HANDLE.lock() else {
        warn!("tee-vault HANDLE poisoned; the namespace TEE key will not be handed out");
        return;
    };
    if slot.as_ref().is_some_and(|abort| !abort.is_finished()) {
        debug!("tee-vault task already running; skipping re-spawn");
        return;
    }
    *slot = Some(
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(SWEEP);
            loop {
                let _ = tick.tick().await;
                sweep(&store, &node_client, &ack_router).await;
            }
        })
        .abort_handle(),
    );
}

/// Abort the running task; [`spawn`] may then rebind it.
pub(crate) fn shutdown() {
    if let Some(abort) = HANDLE.lock().ok().and_then(|mut slot| slot.take()) {
        abort.abort();
    }
}

async fn sweep(store: &Store, node_client: &NodeClient, ack_router: &AckRouter) {
    let namespaces = match NamespaceRepository::new(store).participating_namespaces() {
        Ok(namespaces) => namespaces,
        Err(err) => {
            warn!(%err, "tee-vault could not list this node's namespaces");
            return;
        }
    };
    for namespace in namespaces {
        if let Err(err) = share_in(store, node_client, ack_router, &namespace).await {
            warn!(namespace = %hex::encode(namespace.to_bytes()), %err, "tee-vault could not hand out the namespace TEE key");
        }
    }
}

/// Create the namespace TEE key if `namespace` has none, and deliver every key
/// this node holds to each TEE authority without a copy. Only a TEE authority
/// does either.
async fn share_in(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &AckRouter,
    namespace: &ContextGroupId,
) -> EyreResult<()> {
    let Some((me, secret)) = NamespaceRepository::new(store).identity(namespace)? else {
        return Ok(());
    };
    let Some(account) =
        calimero_governance_store::member_account_in_namespace(store, namespace, &me)?
    else {
        return Ok(());
    };
    if MembershipRepository::new(store).role_of(namespace, &account)?
        != Some(GroupMemberRole::ReadOnlyTee)
    {
        return Ok(());
    }
    let authorities = tee_authority_keys_in_namespace(store, namespace)?;
    if !authorities.contains(&me) {
        return Ok(());
    }

    let secret = PrivateKey::from(secret);
    let deliveries = tee_vault_deliveries(store, namespace)?;
    let mut held = tee_vault_keys(store, namespace, &secret)?;
    if deliveries.is_empty() {
        info!(namespace = %hex::encode(namespace.to_bytes()), "creating the namespace TEE key");
        held.push(PrivateKey::random(&mut rand::rng()));
    }
    let held_keys: Vec<PublicKey> = held.iter().map(PrivateKey::public_key).collect();

    for (vault_key, recipient_key) in owed(&deliveries, &held_keys, &authorities, &me) {
        let vault = held
            .iter()
            .find(|key| key.public_key() == vault_key)
            .ok_or_else(|| eyre!("a held namespace TEE key went missing"))?;
        let envelope = seal_tee_vault_key(vault, &recipient_key)?;
        let report = sign_apply_and_publish(
            store,
            node_client,
            ack_router,
            namespace,
            &secret,
            GroupOp::TeeVaultKeyDelivered {
                vault_key,
                recipient_key,
                envelope,
            },
        )
        .await?;
        report.observe("tee_vault", "TeeVaultKeyDelivered");
        info!(%vault_key, %recipient_key, "delivered the namespace TEE key");
    }
    Ok(())
}

/// The `(key, recipient)` copies this node owes: each key it holds, for each TEE
/// authority the log has no copy of that key for. Copies for this node come
/// first, so a key it has just created is on the log before anyone else holds it.
fn owed(
    deliveries: &[TeeVaultDelivery],
    held: &[PublicKey],
    authorities: &[PublicKey],
    me: &PublicKey,
) -> Vec<(PublicKey, PublicKey)> {
    let mut recipients: Vec<PublicKey> = authorities.to_vec();
    recipients.sort_by_key(|key| key != me);
    let mut owed = Vec::new();
    for vault in held {
        for recipient in &recipients {
            let delivered = deliveries
                .iter()
                .any(|d| d.vault_key == *vault && d.recipient_key == *recipient);
            if !delivered {
                owed.push((*vault, *recipient));
            }
        }
    }
    owed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(byte: u8) -> PublicKey {
        PublicKey::from([byte; 32])
    }

    fn delivery(vault: u8, recipient: u8) -> TeeVaultDelivery {
        TeeVaultDelivery {
            vault_key: key(vault),
            recipient_key: key(recipient),
            envelope: Vec::new(),
        }
    }

    #[test]
    fn a_new_key_goes_to_its_creator_first_then_to_everyone() {
        let owed = owed(&[], &[key(9)], &[key(1), key(2), key(3)], &key(2));
        assert_eq!(
            owed,
            vec![(key(9), key(2)), (key(9), key(1)), (key(9), key(3))]
        );
    }

    #[test]
    fn only_missing_copies_are_owed() {
        let deliveries = [delivery(9, 1), delivery(9, 2)];
        let owed = owed(&deliveries, &[key(9)], &[key(1), key(2), key(3)], &key(1));
        assert_eq!(owed, vec![(key(9), key(3))]);
    }

    /// Two TEEs that each created a key before seeing the other's hand both on,
    /// so both end up holding both.
    #[test]
    fn every_held_key_is_handed_on() {
        let deliveries = [delivery(8, 1), delivery(9, 2)];
        let owed = owed(&deliveries, &[key(8), key(9)], &[key(1), key(2)], &key(1));
        assert_eq!(owed, vec![(key(8), key(2)), (key(9), key(1))]);
    }
}
