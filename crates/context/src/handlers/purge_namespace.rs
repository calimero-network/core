//! Takes a namespace and everything in it off this node's disk, publishing
//! nothing; shared by every handler that ends this node's part in one.

use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{
    CapabilitiesRepository, CascadePayload, MetadataRepository, NamespaceRepository,
};
use calimero_node_primitives::client::NodeClient;
use calimero_store::Store;
use tracing::warn;

use crate::handlers::delete_context::purge_context_with_dag;

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
        let parent = NamespaceRepository::new(datastore).parent(&group)?;
        calimero_governance_store::delete_group_local_rows(datastore, &group)?;
        if let Some(parent) = parent {
            let mut handle = datastore.handle();
            handle.delete(&calimero_store::key::GroupParentRef::new(group.to_bytes()))?;
            handle.delete(&calimero_store::key::GroupChildIndex::new(
                parent.to_bytes(),
                group.to_bytes(),
            ))?;
        }
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
