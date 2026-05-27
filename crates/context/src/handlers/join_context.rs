use calimero_governance_store::{MembershipRepository, MetaRepository, NamespaceRepository};
use std::time::Duration;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{JoinContextRequest, JoinContextResponse};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::ContextConfigParams;
use eyre::bail;
use tokio::sync::broadcast::error::RecvError;
use tracing::{info, warn};

use calimero_governance_store::registration_notify;

use crate::ContextManager;

/// Overall budget for the context→group mapping to land locally after a
/// `sync_known_namespaces` kick. Dominated by peer-discovery in the cold
/// case (`Mesh low` / no peers); the normal case wakes within a few ms as
/// soon as `registration_notify::notify` fires from the apply path.
const GROUP_LOOKUP_TIMEOUT: Duration = Duration::from_secs(30);

/// Fallback poll interval in case the notifier channel lags (burst of
/// registrations overflowing the broadcast capacity). Lag is handled by
/// re-reading the datastore; this bounds how long a lagged receiver
/// waits before that recheck.
const FALLBACK_POLL: Duration = Duration::from_millis(200);

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
        let ack_router = std::sync::Arc::clone(&self.ack_router);
        ActorResponse::r#async(
            async move {
                let mut group_id = calimero_governance_store::get_group_for_context(&datastore, &context_id)?;
                if group_id.is_none() {
                    // Subscribe BEFORE kicking sync so we cannot miss a signal
                    // that fires between the sync returning and us starting to
                    // wait. All messages sent after this point are delivered.
                    let mut rx = registration_notify::subscribe();

                    warn!(
                        %context_id,
                        "context->group mapping missing locally; syncing known namespaces"
                    );
                    sync_known_namespaces(&datastore, &node_client).await;

                    // Mapping may have landed synchronously during sync (creator's
                    // own apply, or a sync that completed and applied inline).
                    group_id = calimero_governance_store::get_group_for_context(&datastore, &context_id)?;

                    if group_id.is_none() {
                        let deadline = tokio::time::Instant::now() + GROUP_LOOKUP_TIMEOUT;
                        let started = tokio::time::Instant::now();
                        loop {
                            // Race the notifier against a short poll interval: if
                            // the channel lagged (bursty traffic), we still catch
                            // the mapping via the periodic datastore recheck.
                            let recv = tokio::time::timeout(FALLBACK_POLL, rx.recv()).await;
                            match recv {
                                Ok(Ok(cid)) if cid == context_id => {
                                    group_id = calimero_governance_store::get_group_for_context(
                                        &datastore, &context_id,
                                    )?;
                                    if group_id.is_some() {
                                        info!(
                                            %context_id,
                                            elapsed_ms = started.elapsed().as_millis() as u64,
                                            "resolved context->group mapping via registration signal"
                                        );
                                        break;
                                    }
                                }
                                Ok(Ok(_)) => {
                                    // Signal for a different context — keep waiting.
                                }
                                Ok(Err(RecvError::Lagged(skipped))) => {
                                    warn!(
                                        %context_id,
                                        skipped,
                                        "registration_notify lagged; falling back to datastore poll"
                                    );
                                    group_id = calimero_governance_store::get_group_for_context(
                                        &datastore, &context_id,
                                    )?;
                                    if group_id.is_some() {
                                        break;
                                    }
                                }
                                Ok(Err(RecvError::Closed)) => {
                                    // Channel sender dropped; final datastore check then bail.
                                    group_id = calimero_governance_store::get_group_for_context(
                                        &datastore, &context_id,
                                    )?;
                                    break;
                                }
                                Err(_elapsed) => {
                                    // Poll tick — recheck the datastore and kick another
                                    // namespace sync to cover the "peer arrived late" case.
                                    group_id = calimero_governance_store::get_group_for_context(
                                        &datastore, &context_id,
                                    )?;
                                    if group_id.is_some() {
                                        break;
                                    }
                                    sync_known_namespaces(&datastore, &node_client).await;
                                }
                            }
                            if tokio::time::Instant::now() >= deadline {
                                break;
                            }
                        }
                    }
                }

                let group_id =
                    group_id.ok_or_else(|| eyre::eyre!("context does not belong to any group"))?;

                // Resolve joiner identity from node namespace identity.
                let (joiner_identity, _) =
                    NamespaceRepository::new(&datastore).resolve_identity(&group_id)?
                        .map(|(pk, sk, _sender)| (pk, sk))
                        .ok_or_else(|| {
                            eyre::eyre!(
                            "node has no namespace identity for this group; join the group first"
                        )
                        })?;

                // Group membership covers both direct members and parent-chain
                // members inherited through `Open` subgroups (gated by the
                // `CAN_JOIN_OPEN_SUBGROUPS` capability at the anchor parent).
                // `Restricted` subgroups still require an explicit
                // `add_group_members` call by an admin.
                if MetaRepository::new(&datastore).load(&group_id)?.is_none() {
                    bail!("group not found");
                }
                let membership_path = MembershipRepository::new(&datastore).check_path(&group_id, &joiner_identity, )?;
                let mut was_inherited = false;
                match membership_path {
                    calimero_governance_store::MembershipPath::None => {
                        bail!("identity is not a member of the group");
                    }
                    calimero_governance_store::MembershipPath::Direct => {}
                    calimero_governance_store::MembershipPath::Inherited { anchor, via_admin } => {
                        // Audit trail: inherited members do not appear in
                        // `list_group_members` for the subgroup, so emit a
                        // structured log so admins can reconstruct who has
                        // access via the parent-walk inheritance path
                        // (issue #2256).
                        info!(
                            target: "calimero::audit::group_membership",
                            subgroup_id = %hex::encode(group_id.to_bytes()),
                            anchor_parent = %hex::encode(anchor.to_bytes()),
                            %joiner_identity,
                            %context_id,
                            via_admin,
                            "context join authorized via inherited subgroup membership"
                        );
                        was_inherited = true;
                    }
                }

                let ns_id = NamespaceRepository::new(&datastore).resolve(&group_id)?;
                let ns_identity = NamespaceRepository::new(&datastore).identity(&ns_id)?
                    .ok_or_else(|| eyre::eyre!("namespace identity not found"))?;
                let (_pk, sk_bytes, _sender) = ns_identity;

                let zero_app = calimero_primitives::application::ApplicationId::from([0u8; 32]);
                let config = if !context_client.has_context(&context_id)? {
                    let app_id = MetaRepository::new(&datastore).load(&group_id)?
                        .map(|meta| meta.target_application_id)
                        .filter(|id| *id != zero_app);

                    // Read service_name from the dedicated context service name key,
                    // written during ContextRegistered governance application.
                    let svc_name = calimero_governance_store::get_context_service_name(&datastore, &context_id)?;

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

                    // Clear any leave-tombstone written by a previous
                    // `leave_context` for this `(member, context)` pair —
                    // explicit rejoin means the user is opting back in, so
                    // auto-follow should not see the marker on future events.
                    let marker_key = calimero_store::key::ContextLeftMarker::new(
                        context_id,
                        joiner_identity,
                    );
                    if let Err(err) = handle.delete(&marker_key) {
                        warn!(
                            %context_id,
                            ?err,
                            "join_context: failed to clear leave marker — \
                             auto-follow may continue to skip this context until cleared"
                        );
                    }
                }

                node_client.subscribe(&context_id).await?;
                node_client.sync(Some(&context_id), None).await?;

                info!(
                    ?group_id,
                    ?context_id,
                    %joiner_identity,
                    "joined context via group membership"
                );

                // Inherited members (Open subgroup, joined via the
                // CAN_JOIN_OPEN_SUBGROUPS parent-walk) don't get a
                // `KeyDelivery` from any admin — admin never called
                // `add_group_members` for them. Without the group key
                // their `sender_key` stays None on the ContextIdentity
                // row above, which means they can't decrypt state-DAG
                // messages and others can't decrypt theirs. Publish
                // `RootOp::MemberJoinedOpen` signed by us so any peer
                // holding the key responds with a `KeyDelivery` (same
                // mechanism `MemberJoined` uses for invitation-based
                // joins).
                //
                // Direct members are explicit-add via
                // `add_group_members` and already get a `KeyDelivery`
                // emitted alongside their `MemberAdded` — skip this
                // path for them.
                if was_inherited {
                    let signer_sk = calimero_primitives::identity::PrivateKey::from(sk_bytes);
                    let op = calimero_context_client::local_governance::NamespaceOp::Root(
                        calimero_context_client::local_governance::RootOp::MemberJoinedOpen {
                            member: joiner_identity,
                            group_id: group_id.to_bytes(),
                        },
                    );
                    if let Err(e) = calimero_governance_store::sign_apply_and_publish_namespace_op(
                        &datastore,
                        &node_client,
                        &ack_router,
                        ns_id.to_bytes(),
                        &signer_sk,
                        op,
                    )
                    .await
                    {
                        warn!(
                            ?e,
                            %joiner_identity,
                            %context_id,
                            "failed to publish MemberJoinedOpen — key delivery to this \
                             inherited joiner will be skipped; messages will appear local-only \
                             until an admin explicitly add_group_members the joiner"
                        );
                    }
                }

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
    let groups = match MetaRepository::new(datastore).enumerate_all(0, usize::MAX) {
        Ok(groups) => groups,
        Err(err) => {
            warn!(error = ?err, "failed to enumerate groups for namespace sync");
            return;
        }
    };

    for (group_id_bytes, _) in groups {
        let group_id = ContextGroupId::from(group_id_bytes);
        let namespace = match NamespaceRepository::new(datastore).resolve(&group_id) {
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
