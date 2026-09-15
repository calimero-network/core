//! Taking part in a namespace and listening on it - one definition, shared by
//! `pair-init` and the account-follow listener.
//!
//! Node-local and idempotent on both halves, so following twice, or following
//! one this node already takes part in, costs nothing. Participation is not
//! cosmetic: the sync layer and the startup sweep walk it, so a namespace
//! without a row is one this node never syncs, and one without a subscription is
//! one it never hears a beacon on.

use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::NamespaceRepository;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use eyre::Result as EyreResult;
use tracing::warn;

/// Take part in `namespace`, subscribe to its topic and pull it, answering
/// with this node's signing identity for it.
///
/// The pull is best-effort and only logged on failure, same as `pair_device_init`
/// treats it today: gossip alone can leave a namespace's first op pending forever.
///
/// # Errors
/// Propagates the identity provisioning or the subscribe failure.
pub(crate) async fn follow(
    store: &Store,
    node_client: &NodeClient,
    namespace: &ContextGroupId,
) -> EyreResult<(PublicKey, [u8; 32])> {
    let (_namespace, sign_pk, sign_sk) = NamespaceRepository::new(store)
        .participate_in(namespace)
        .map_err(|err| {
            eyre::eyre!("failed to provision a namespace identity for {namespace:?}: {err}")
        })?;
    node_client
        .subscribe_namespace(namespace.to_bytes())
        .await?;
    // Every other subscriber pulls straight after subscribing:
    // gossip only carries what is published from now on.
    if let Err(err) = node_client.sync_namespace(namespace.to_bytes()).await {
        warn!(?namespace, %err, "failed to queue the namespace governance pull");
    }
    Ok((sign_pk, sign_sk))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::NamespaceRepository;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;

    use super::follow;
    use crate::test_support::actor;

    const NS: [u8; 32] = [0xB1; 32];

    /// Both halves are load-bearing, and both are idempotent: a namespace with no
    /// participation row is one the sync layer never walks, and one with no
    /// subscription is one this node never hears anything on. The idempotence is
    /// pinned on the participation rows - the topic set dedupes above this.
    #[actix::test]
    async fn following_a_namespace_notes_participation_and_subscribes() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let mut harness = actor::over(store.clone()).await;
        let namespace = ContextGroupId::from(NS);

        for _ in 0..2 {
            let _identity = follow(&store, &harness.node_client, &namespace)
                .await
                .expect("following is node-local and publishes nothing");
        }

        assert_eq!(
            NamespaceRepository::new(&store)
                .participating_namespaces()
                .expect("read the participation rows"),
            vec![namespace],
            "following twice takes part once"
        );
        assert_eq!(
            harness.subscribed(),
            vec![format!("ns/{}", hex::encode(NS))],
            "and subscribes once: `TopicManager` short-circuits a topic it holds"
        );
    }
}
