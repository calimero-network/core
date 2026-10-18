use std::collections::BTreeSet;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{
    ActivateNamespaceUpgradeRequest, ActivateNamespaceUpgradeResponse, ActivateUpgradeRequest,
};
use calimero_governance_store::{
    find_local_signing_identity, get_group_for_context, MetaRepository, NamespaceRepository,
};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use eyre::{bail, WrapErr};
use tracing::debug;

use crate::error::ContextError;
use crate::ContextManager;

/// What an activation switches, read from the store before any context moves.
#[derive(Debug)]
struct ActivationPlan {
    target_application_id: ApplicationId,
    contexts: Vec<(ContextId, PublicKey)>,
}

impl Handler<ActivateNamespaceUpgradeRequest> for ContextManager {
    type Result = ActorResponse<Self, <ActivateNamespaceUpgradeRequest as Message>::Result>;

    fn handle(
        &mut self,
        request: ActivateNamespaceUpgradeRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let plan = match plan_activation(
            &self.datastore,
            |blob| self.node_client.has_blob(blob),
            &request,
        ) {
            Ok(plan) => plan,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        let context_client = self.context_client.clone();
        let node_client = self.node_client.clone();
        let run = async move {
            let mut response = ActivateNamespaceUpgradeResponse {
                target_application_id: plan.target_application_id,
                switched_contexts: 0,
                replayed_deltas: 0,
            };
            for (context_id, executor) in plan.contexts {
                let switched = context_client
                    .activate_upgrade(ActivateUpgradeRequest {
                        context_id,
                        executor,
                    })
                    .await
                    .wrap_err_with(|| format!("switching context {context_id}"))?;
                response.switched_contexts += u32::from(switched);

                // Also for a context that was already current: its held deltas
                // may have been left unreplayed by an earlier lazy switch.
                let replayed = node_client
                    .drain_absorbed(context_id)
                    .await
                    .wrap_err_with(|| format!("replaying held deltas of context {context_id}"))?;
                response.replayed_deltas = response
                    .replayed_deltas
                    .saturating_add(u32::try_from(replayed).unwrap_or(u32::MAX));
            }
            Ok(response)
        };

        ActorResponse::r#async(run.into_actor(self))
    }
}

/// Resolve the namespace's stored target and the contexts this node can switch.
///
/// Refuses when a target blob a switch needs is not local, so the switch can
/// never reach for a release that was not installed first.
fn plan_activation(
    store: &Store,
    has_blob: impl Fn(&BlobId) -> eyre::Result<bool>,
    request: &ActivateNamespaceUpgradeRequest,
) -> eyre::Result<ActivationPlan> {
    let namespace_id = &request.namespace_id;
    let Some(meta) = MetaRepository::new(store).load(namespace_id)? else {
        bail!(ContextError::NamespaceNotFound {
            namespace_id: namespace_id.to_string(),
        });
    };
    let target_application_id = meta.target.application_id;
    if let Some(expected) = request.expected_application_id {
        if expected != target_application_id {
            bail!(ContextError::UpgradeTargetMismatch {
                namespace_id: namespace_id.to_string(),
                expected: expected.to_string(),
                actual: target_application_id.to_string(),
            });
        }
    }

    let mut contexts = Vec::new();
    let mut target_blobs = BTreeSet::new();
    for context_id in NamespaceRepository::new(store)
        .collect_subtree_for_cascade(namespace_id)?
        .contexts
    {
        let Some(executor) = find_local_signing_identity(store, &context_id)? else {
            debug!(%context_id, "no local identity in the context; not switching it");
            continue;
        };
        if let Some(group_id) = get_group_for_context(store, &context_id)? {
            if let Some(group) = MetaRepository::new(store).load(&group_id)? {
                // A zero id names no blob, as in the lazy upgrade's own check.
                if group.target.bytecode_id != [0u8; 32] {
                    let _new = target_blobs.insert(group.target.bytecode_id);
                }
            }
        }
        contexts.push((context_id, executor));
    }

    for bytecode_id in target_blobs {
        let blob_id = BlobId::from(bytecode_id);
        if !has_blob(&blob_id)? {
            bail!(ContextError::BytecodeNotInstalled {
                blob_id: blob_id.to_string(),
            });
        }
    }

    Ok(ActivationPlan {
        target_application_id,
        contexts,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::register_context_in_group;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::{GroupMetaValue, GroupTarget};

    use super::*;

    const TARGET_BLOB: [u8; 32] = [0x02; 32];

    fn store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    fn app() -> ApplicationId {
        ApplicationId::from([0xAA; 32])
    }

    fn seed_group(store: &Store, id: u8) -> ContextGroupId {
        seed_group_targeting(store, id, app())
    }

    fn seed_group_targeting(
        store: &Store,
        id: u8,
        application_id: ApplicationId,
    ) -> ContextGroupId {
        let gid = ContextGroupId::from([id; 32]);
        let admin = PublicKey::from([0x07; 32]);
        MetaRepository::new(store)
            .save(
                &gid,
                &GroupMetaValue {
                    target: GroupTarget {
                        application_id,
                        bytecode_id: TARGET_BLOB,
                        package: Box::default(),
                        version: Box::default(),
                    },
                    created_at: 0,
                    admin_identity: crate::test_support::account_for(&admin),
                    owner_identity: crate::test_support::account_for(&admin),
                    migration: None,
                    auto_join: false,
                },
            )
            .unwrap();
        gid
    }

    fn seed_context(store: &Store, group: &ContextGroupId, id: u8, local: bool) -> ContextId {
        let context_id = ContextId::from([id; 32]);
        register_context_in_group(store, group, &context_id).unwrap();
        if local {
            store
                .handle()
                .put(
                    &calimero_store::key::ContextIdentity::new(
                        context_id,
                        PublicKey::from([id; 32]),
                    ),
                    &calimero_store::types::ContextIdentity {
                        private_key: Some([0x77; 32]),
                    },
                )
                .unwrap();
        }
        context_id
    }

    fn request(
        namespace: ContextGroupId,
        expected: Option<ApplicationId>,
    ) -> ActivateNamespaceUpgradeRequest {
        ActivateNamespaceUpgradeRequest {
            namespace_id: namespace,
            expected_application_id: expected,
        }
    }

    fn installed(_: &BlobId) -> eyre::Result<bool> {
        Ok(true)
    }

    fn refusal(err: &eyre::Report) -> &ContextError {
        err.downcast_ref::<ContextError>().expect("a typed refusal")
    }

    #[test]
    fn an_unknown_namespace_is_refused_as_not_found() {
        let err = plan_activation(
            &store(),
            installed,
            &request(ContextGroupId::from([0x10; 32]), None),
        )
        .unwrap_err();
        assert!(matches!(
            refusal(&err),
            ContextError::NamespaceNotFound { .. }
        ));
    }

    #[test]
    fn an_expected_application_that_differs_from_the_target_is_refused() {
        let store = store();
        let ns = seed_group(&store, 0x10);
        let err = plan_activation(
            &store,
            installed,
            &request(ns, Some(ApplicationId::from([0xBB; 32]))),
        )
        .unwrap_err();
        assert!(matches!(
            refusal(&err),
            ContextError::UpgradeTargetMismatch { .. }
        ));
    }

    #[test]
    fn a_subgroup_is_refused_as_not_a_namespace_root() {
        let store = store();
        let ns = seed_group(&store, 0x10);
        let sub = seed_group(&store, 0x11);
        NamespaceRepository::new(&store).nest(&ns, &sub).unwrap();
        let _ctx = seed_context(&store, &sub, 0x22, true);

        let err = plan_activation(&store, installed, &request(sub, None)).unwrap_err();
        assert!(matches!(
            refusal(&err),
            ContextError::NotANamespaceRoot { .. }
        ));
    }

    #[test]
    fn a_subgroup_targeting_another_application_than_expected_is_refused() {
        let store = store();
        let ns = seed_group(&store, 0x10);
        let sub = seed_group_targeting(&store, 0x11, ApplicationId::from([0xBB; 32]));
        NamespaceRepository::new(&store).nest(&ns, &sub).unwrap();
        let _root_ctx = seed_context(&store, &ns, 0x21, true);
        let _sub_ctx = seed_context(&store, &sub, 0x22, true);

        let err = plan_activation(&store, installed, &request(ns, Some(app()))).unwrap_err();
        assert!(matches!(
            refusal(&err),
            ContextError::UpgradeTargetMismatch { .. }
        ));
    }

    #[test]
    fn the_stored_target_and_every_local_context_of_the_subtree_are_planned() {
        let store = store();
        let ns = seed_group(&store, 0x10);
        let sub = seed_group(&store, 0x11);
        NamespaceRepository::new(&store).nest(&ns, &sub).unwrap();
        let root_ctx = seed_context(&store, &ns, 0x21, true);
        let sub_ctx = seed_context(&store, &sub, 0x22, true);
        let _remote = seed_context(&store, &sub, 0x23, false);

        for expected in [None, Some(app())] {
            let plan = plan_activation(&store, installed, &request(ns, expected)).unwrap();
            assert_eq!(plan.target_application_id, app());
            let mut planned: Vec<_> = plan.contexts.iter().map(|(id, _)| *id).collect();
            planned.sort();
            assert_eq!(planned, vec![root_ctx, sub_ctx]);
        }
    }

    #[test]
    fn a_target_blob_that_is_not_local_is_refused_rather_than_fetched() {
        let store = store();
        let ns = seed_group(&store, 0x10);
        let _ctx = seed_context(&store, &ns, 0x21, true);

        let err = plan_activation(&store, |_| Ok(false), &request(ns, None)).unwrap_err();
        assert!(matches!(
            refusal(&err),
            ContextError::BytecodeNotInstalled { .. }
        ));
    }

    #[test]
    fn a_namespace_with_nothing_local_to_switch_plans_no_contexts() {
        let store = store();
        let ns = seed_group(&store, 0x10);
        let _remote = seed_context(&store, &ns, 0x21, false);

        let plan = plan_activation(&store, |_| Ok(false), &request(ns, None)).unwrap();
        assert!(plan.contexts.is_empty());
    }
}
