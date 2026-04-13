use std::time::Duration;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{JoinContextRequest, JoinContextResponse};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::ContextConfigParams;
use eyre::bail;
use tracing::{info, warn};

use crate::{group_store, ContextManager};

/// Per-attempt sleep schedule for resolving the context->group mapping.
///
/// The mapping is delivered by the `ContextRegistered` governance op, which
/// propagates asynchronously over gossipsub from the creating node.
///
/// Exponential-ish backoff is used so we:
/// 1. Wake fast for the common case where the op arrives within a few hundred
///    ms of `join_context` being called (CI evidence: ops observed arriving
///    ~40ms after the previous flat 1.8s budget expired).
/// 2. Still cover the slow-propagation tail (~10s total budget) without
///    waiting the full budget on every successful join.
///
/// Total worst-case wait ≈ 150ms + 400ms + 1s + 2.5s + 6s = ~10s.
const GROUP_LOOKUP_BACKOFF: &[Duration] = &[
    Duration::from_millis(150),
    Duration::from_millis(400),
    Duration::from_secs(1),
    Duration::from_millis(2500),
    Duration::from_secs(6),
];

impl Handler<JoinContextRequest> for ContextManager {
    type Result = ActorResponse<Self, <JoinContextRequest as Message>::Result>;

    fn handle(
        &mut self,
        JoinContextRequest { context_id }: JoinContextRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let datastore = self.datastore.clone();
        let context_client = self.context_client.clone();
        let node_client = self.node_client.clone();
        ActorResponse::r#async(
            async move {
                let mut group_id = group_store::get_group_for_context(&datastore, &context_id)?;
                if group_id.is_none() {
                    warn!(
                        %context_id,
                        "context->group mapping missing locally; syncing known namespaces"
                    );
                    sync_known_namespaces(&datastore, &node_client).await;

                    for (attempt, delay) in GROUP_LOOKUP_BACKOFF.iter().enumerate() {
                        tokio::time::sleep(*delay).await;
                        group_id = group_store::get_group_for_context(&datastore, &context_id)?;
                        if group_id.is_some() {
                            info!(
                                %context_id,
                                attempt = attempt + 1,
                                "resolved context->group mapping after namespace sync"
                            );
                            break;
                        }
                        sync_known_namespaces(&datastore, &node_client).await;
                    }
                }

                let group_id =
                    group_id.ok_or_else(|| eyre::eyre!("context does not belong to any group"))?;

                // Resolve joiner identity from node namespace identity.
                let (joiner_identity, _) =
                    group_store::resolve_namespace_identity(&datastore, &group_id)?
                        .map(|(pk, sk, _sender)| (pk, sk))
                        .ok_or_else(|| {
                            eyre::eyre!(
                            "node has no namespace identity for this group; join the group first"
                        )
                        })?;

                // Group membership already verified above. All contexts in a group
                // a member has access to are joinable. Restricted access is handled
                // at the subgroup level (admin must explicitly add member to the subgroup).
                if group_store::load_group_meta(&datastore, &group_id)?.is_none() {
                    bail!("group not found");
                }
                if !group_store::check_group_membership(&datastore, &group_id, &joiner_identity)? {
                    bail!("identity is not a member of the group");
                }

                let ns_id = group_store::resolve_namespace(&datastore, &group_id)?;
                let ns_identity = group_store::get_namespace_identity(&datastore, &ns_id)?
                    .ok_or_else(|| eyre::eyre!("namespace identity not found"))?;
                let (_pk, sk_bytes, _sender) = ns_identity;

                let zero_app = calimero_primitives::application::ApplicationId::from([0u8; 32]);
                let config = if !context_client.has_context(&context_id)? {
                    let app_id = group_store::load_group_meta(&datastore, &group_id)?
                        .map(|meta| meta.target_application_id)
                        .filter(|id| *id != zero_app);

                    // Read service_name from the dedicated context service name key,
                    // written during ContextRegistered governance application.
                    let svc_name = group_store::get_context_service_name(&datastore, &context_id)?;

                    Some(ContextConfigParams {
                        application_id: app_id,
                        application_revision: 0,
                        members_revision: 0,
                        service_name: svc_name,
                    })
                } else {
                    None
                };

                let _ignored = context_client
                    .sync_context_config(context_id, config)
                    .await?;

                {
                    let mut handle = datastore.handle();
                    handle.put(
                        &calimero_store::key::ContextIdentity::new(context_id, joiner_identity),
                        &calimero_store::types::ContextIdentity {
                            private_key: Some(sk_bytes),
                            sender_key: None,
                        },
                    )?;
                }

                node_client.subscribe(&context_id).await?;
                node_client.sync(Some(&context_id), None).await?;

                info!(
                    ?group_id,
                    ?context_id,
                    %joiner_identity,
                    "joined context via group membership"
                );

                Ok(JoinContextResponse {
                    context_id,
                    member_public_key: joiner_identity,
                })
            }
            .into_actor(self),
        )
    }
}

async fn sync_known_namespaces(
    datastore: &calimero_store::Store,
    node_client: &calimero_node_primitives::client::NodeClient,
) {
    let groups = match group_store::enumerate_all_groups(datastore, 0, usize::MAX) {
        Ok(groups) => groups,
        Err(err) => {
            warn!(error = ?err, "failed to enumerate groups for namespace sync");
            return;
        }
    };

    for (group_id_bytes, _) in groups {
        let group_id = ContextGroupId::from(group_id_bytes);
        let namespace = match group_store::resolve_namespace(datastore, &group_id) {
            Ok(namespace) => namespace,
            Err(err) => {
                warn!(?group_id, error = ?err, "failed to resolve namespace while syncing");
                continue;
            }
        };

        let namespace_id = namespace.to_bytes();
        if let Err(err) = node_client.subscribe_namespace(namespace_id).await {
            warn!(?group_id, error = ?err, "failed to subscribe namespace during join_context");
        }
        if let Err(err) = node_client.sync_namespace(namespace_id).await {
            warn!(?group_id, error = ?err, "failed to sync namespace during join_context");
        }
    }
}
