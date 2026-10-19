//! Takes a namespace and everything in it off this node's disk, publishing
//! nothing; shared by every handler that ends this node's part in one.

use actix::{ActorFutureExt, ActorResponse, Handler, Message, ResponseActFuture, WrapFuture};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{
    CapabilitiesRepository, CascadePayload, MetadataRepository, NamespaceRepository,
};
use calimero_node_primitives::client::NodeClient;
use calimero_store::Store;
use either::Either;
use tracing::{info, warn};

use crate::handlers::delete_context::purge_context_with_dag;
use crate::ContextManager;

/// Take a namespace root off this node, for a leave that asked every device of
/// the account to. Crate-internal: it publishes nothing and checks no membership.
#[derive(Debug, Message)]
#[rtype(result = "eyre::Result<()>")]
pub(crate) struct PurgeNamespace {
    pub namespace: ContextGroupId,
}

impl Handler<PurgeNamespace> for ContextManager {
    type Result = ActorResponse<Self, eyre::Result<()>>;

    fn handle(
        &mut self,
        PurgeNamespace { namespace }: PurgeNamespace,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let repository = NamespaceRepository::new(&self.datastore);
        let payload = match repository.resolve(&namespace).and_then(|resolved| {
            eyre::ensure!(
                resolved == namespace,
                "{namespace:?} is not a namespace root"
            );
            repository.collect_subtree_for_cascade(&namespace)
        }) {
            Ok(payload) => payload,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        ActorResponse::r#async(self.purge_subtree(namespace, payload))
    }
}

impl ContextManager {
    /// [`purge_namespace`] with every cached context of `payload` locked, so no
    /// run writes while its rows go; the purged contexts then leave the cache.
    pub(crate) fn purge_subtree(
        &self,
        namespace: ContextGroupId,
        payload: CascadePayload,
    ) -> ResponseActFuture<Self, eyre::Result<()>> {
        let locks: Vec<_> = payload
            .contexts
            .iter()
            .filter_map(|context| self.contexts.get(context))
            .map(|context| context.lock())
            .collect();
        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();
        let search = self.search.clone();

        let task = async move {
            let mut _held = Vec::with_capacity(locks.len());
            for lock in locks {
                _held.push(match lock {
                    Either::Left(guard) => guard,
                    Either::Right(task) => task.await,
                });
            }
            let outcome = purge_namespace(
                &datastore,
                &node_client,
                search.as_deref(),
                &namespace,
                &payload,
            )
            .await;
            if outcome.is_ok() {
                info!(
                    ?namespace,
                    total_groups = payload.descendant_groups.len() + 1,
                    total_contexts = payload.contexts.len(),
                    "removed namespace and subtree from this node"
                );
            }
            (payload.contexts, outcome)
        };

        // Dropped on failure too: a purge cut short leaves cached state over missing rows.
        Box::pin(task.into_actor(self).map(|(contexts, outcome), act, _ctx| {
            for context in contexts {
                let _ignored = act.contexts.remove(&context);
            }
            outcome
        }))
    }
}

/// Every context and group of `payload` (the namespace's subtree), and its
/// namespace-level state, gone from this node; the topics dropped.
///
/// # Errors
/// A store failure; the namespace identity goes last, so a rerun finishes the job.
pub(crate) async fn purge_namespace(
    datastore: &Store,
    node_client: &NodeClient,
    search: Option<&calimero_search::SearchService>,
    namespace: &ContextGroupId,
    payload: &CascadePayload,
) -> eyre::Result<()> {
    // First, so gossip cannot write rows back while they are removed.
    if let Err(err) = node_client
        .unsubscribe_namespace(namespace.to_bytes())
        .await
    {
        warn!(?namespace, %err, "failed to unsubscribe a purged namespace");
    }
    for context in &payload.contexts {
        // Close the index before the purge takes its files away.
        if let Some(search) = search {
            search.delete_context(context.as_ref())?;
        }
        if let Err(err) = node_client.unsubscribe(context).await {
            warn!(%context, %err, "failed to unsubscribe a context of a purged namespace");
        }
        purge_context_with_dag(datastore, context)?;
        if let Err(err) = node_client.release_prefetched_blobs(context).await {
            warn!(%context, %err, "failed to release the blobs prefetched for the context");
        }
    }

    // Children first, the namespace root last.
    let groups = payload
        .descendant_groups
        .iter()
        .copied()
        .chain(std::iter::once(*namespace));
    for group in groups {
        for context in
            calimero_governance_store::enumerate_group_contexts(datastore, &group, 0, usize::MAX)?
        {
            calimero_governance_store::unregister_context_from_group(datastore, &group, &context)?;
            MetadataRepository::new(datastore).delete_context(&group, &context)?;
            CapabilitiesRepository::new(datastore).delete_context_members(&group, &context)?;
        }
        calimero_governance_store::delete_group_and_parent_edge(datastore, &group)?;
    }
    calimero_governance_store::delete_namespace_local_state(datastore, namespace)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::test_fixtures::test_meta;
    use calimero_governance_store::{
        register_context_in_group, MetaRepository, NamespaceRepository,
    };
    use calimero_store::db::{Column, InMemoryDB};
    use calimero_store::Store;

    use super::*;
    use crate::test_support::actor;

    const NS: [u8; 32] = [0xA0; 32];
    const SUB: [u8; 32] = [0xA1; 32];
    const OTHER_NS: [u8; 32] = [0xB0; 32];
    const CTX_ROOT: [u8; 32] = [0x11; 32];
    const CTX_SUB: [u8; 32] = [0x12; 32];
    const CTX_OTHER: [u8; 32] = [0x13; 32];

    /// A state, delta and identity row for `context`.
    fn context_rows(context: [u8; 32]) -> [(Column, Vec<u8>); 3] {
        let keyed = |tail: u8| [context.to_vec(), vec![tail; 32]].concat();
        [
            (Column::State, keyed(1)),
            (Column::Delta, keyed(2)),
            (Column::Identity, keyed(3)),
        ]
    }

    fn present(store: &Store, (column, key): &(Column, Vec<u8>)) -> bool {
        store.raw_get(*column, key).expect("read").is_some()
    }

    /// A namespace topic is `ns/<hex>`; a context's is the bare hex.
    fn context_topic(id: [u8; 32]) -> String {
        hex::encode(id)
    }

    #[actix::test]
    async fn purging_a_namespace_leaves_nothing_of_its_subtree_and_spares_every_other_namespace() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let mut harness = actor::over(store.clone()).await;
        let (root, sub, other) = (
            ContextGroupId::from(NS),
            ContextGroupId::from(SUB),
            ContextGroupId::from(OTHER_NS),
        );
        let namespaces = NamespaceRepository::new(&store);
        for group in [root, sub, other] {
            MetaRepository::new(&store)
                .save(&group, &test_meta())
                .expect("save the group");
        }
        for namespace in [root, other] {
            let _identity = namespaces.participate_in(&namespace).expect("take part");
        }
        namespaces.nest(&root, &sub).expect("nest the subgroup");
        for (group, context) in [(root, CTX_ROOT), (sub, CTX_SUB), (other, CTX_OTHER)] {
            register_context_in_group(&store, &group, &context.into()).expect("register");
            for (column, key) in context_rows(context) {
                store.raw_put(column, &key, b"value").expect("write");
            }
        }
        let payload = namespaces
            .collect_subtree_for_cascade(&root)
            .expect("collect the subtree");

        purge_namespace(&store, &harness.node_client, None, &root, &payload)
            .await
            .expect("purge");

        for context in [CTX_ROOT, CTX_SUB] {
            for row in context_rows(context) {
                assert!(
                    !present(&store, &row),
                    "{:?} row of a purged context",
                    row.0
                );
            }
        }
        for row in context_rows(CTX_OTHER) {
            assert!(
                present(&store, &row),
                "{:?} row of another namespace",
                row.0
            );
        }
        for group in [root, sub] {
            assert!(MetaRepository::new(&store).load(&group).unwrap().is_none());
        }
        assert!(MetaRepository::new(&store).load(&other).unwrap().is_some());
        assert_eq!(
            namespaces.participating_namespaces().unwrap(),
            vec![other],
            "participation in the purged namespace is gone"
        );
        let dropped = harness.unsubscribed();
        for topic in [
            format!("ns/{}", hex::encode(NS)),
            context_topic(CTX_ROOT),
            context_topic(CTX_SUB),
        ] {
            assert!(dropped.contains(&topic), "{topic} not dropped: {dropped:?}");
        }
        assert!(!dropped.contains(&context_topic(CTX_OTHER)));
    }
}
