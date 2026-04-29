use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{JoinGroupRequest, JoinGroupResponse};
use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
use calimero_primitives::context::{ContextConfigParams, GroupMemberRole};
use calimero_primitives::identity::PrivateKey;
use calimero_store::key;
use tokio::sync::broadcast::error::RecvError;
use tracing::{info, warn};

use crate::op_events::subscribe as subscribe_op_events;
use crate::op_events::OpEvent;
use crate::{group_store, ContextManager};

const NAMESPACE_MESH_GRACE: Duration = Duration::from_secs(2);

// Maximum time `join_group` waits on the gossip-fallback path for a
// `KeyDelivery` op addressed to the joiner now lives on
// [`ContextManagerConfig::key_delivery_fallback_wait`] so operators can
// override it without source patches. Default is preserved (5s).

impl Handler<JoinGroupRequest> for ContextManager {
    type Result = ActorResponse<Self, <JoinGroupRequest as Message>::Result>;

    fn handle(
        &mut self,
        JoinGroupRequest {
            invitation,
            group_alias,
        }: JoinGroupRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let group_id = invitation.invitation.group_id;
        let invited_role = invitation.invitation.invited_role;
        let expiration = invitation.invitation.expiration_timestamp;

        if expiration != 0 {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            if now > expiration {
                return ActorResponse::reply(Err(eyre::eyre!("invitation expired")));
            }
        }

        let (ns_id, joiner_identity, sk_bytes, _) =
            match self.get_or_create_namespace_identity(&group_id) {
                Ok(result) => result,
                Err(err) => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "failed to resolve namespace identity for join: {err}"
                    )));
                }
            };

        let namespace_id = ns_id.to_bytes();
        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();
        let ack_router = Arc::clone(&self.ack_router);
        let context_client = self.context_client.clone();
        let key_delivery_fallback_wait = self.config.key_delivery_fallback_wait;

        ActorResponse::r#async(
            async move {
                let sk = PrivateKey::from(sk_bytes);
                let role = match invited_role {
                    0 => GroupMemberRole::Admin,
                    2 => GroupMemberRole::ReadOnly,
                    _ => GroupMemberRole::Member,
                };

                // -------------------------------------------------------
                // Phase 1: Set up local state.
                // -------------------------------------------------------

                let _ = group_store::store_group_signing_key(
                    &datastore,
                    &group_id,
                    &joiner_identity,
                    &sk_bytes,
                );

                if group_store::load_group_meta(&datastore, &group_id)?.is_none() {
                    let admin_identity = calimero_primitives::identity::PublicKey::from(
                        invitation.invitation.inviter_identity.to_bytes(),
                    );
                    let meta = calimero_store::key::GroupMetaValue {
                        admin_identity,
                        target_application_id:
                            calimero_primitives::application::ApplicationId::from([0u8; 32]),
                        app_key: [0u8; 32],
                        upgrade_policy: calimero_primitives::context::UpgradePolicy::default(),
                        migration: None,
                        created_at: 0,
                        auto_join: true,
                    };
                    group_store::save_group_meta(&datastore, &group_id, &meta)?;

                    // Add the namespace admin to the member list so joining
                    // nodes see the creator in /admin-api/groups/:id/members.
                    // Direct-row check: see joiner-side guard below for
                    // why inheritance-aware `check_group_membership`
                    // would be unsafe here.
                    if !group_store::has_direct_group_member(
                        &datastore,
                        &group_id,
                        &admin_identity,
                    )? {
                        group_store::add_group_member(
                            &datastore,
                            &group_id,
                            &admin_identity,
                            calimero_primitives::context::GroupMemberRole::Admin,
                        )?;
                    }
                }

                // -------------------------------------------------------
                // Phase 2: Subscribe to namespace topic, wait for mesh
                //          formation, then get everything we need via a
                //          single direct stream request to a mesh peer.
                // -------------------------------------------------------

                let _ = node_client.subscribe_namespace(namespace_id).await;
                tokio::time::sleep(NAMESPACE_MESH_GRACE).await;

                let invitation_bytes = borsh::to_vec(&invitation)
                    .map_err(|e| eyre::eyre!("failed to serialize invitation: {e}"))?;

                let join_result = node_client
                    .request_namespace_join(namespace_id, invitation_bytes, joiner_identity)
                    .await?;

                // Unwrap and store the group key.
                if !join_result.has_key() {
                    warn!("join response contained no group key");
                } else if group_store::load_current_group_key(&datastore, &group_id)?.is_some() {
                    info!(
                        ?group_id,
                        "group key already present locally, skipping store from join response"
                    );
                } else {
                    let envelope: calimero_context_client::local_governance::KeyEnvelope =
                        borsh::from_slice(&join_result.key_envelope_bytes)
                            .map_err(|e| eyre::eyre!("failed to deserialize key envelope: {e}"))?;

                    let group_key = group_store::unwrap_group_key(&sk, &envelope)?;
                    group_store::store_group_key(&datastore, &group_id, &group_key)?;
                    info!("received group key via direct join response");
                }

                // Apply governance ops so the local DAG is up to date.
                for op_bytes in &join_result.governance_ops {
                    if let Ok(op) = borsh::from_slice::<SignedNamespaceOp>(op_bytes) {
                        if let Err(e) = context_client.apply_signed_namespace_op(op).await {
                            warn!(?e, "failed to apply governance op from join response");
                        }
                    }
                }

                // Pull any governance ops published during (or just before) the
                // join window that weren't in the join response snapshot.
                // Direct stream request — does not depend on gossip delivery.
                if let Err(e) = node_client.sync_namespace(namespace_id).await {
                    warn!(
                        ?e,
                        "failed to trigger post-join namespace governance pull (non-fatal)"
                    );
                }

                // Issue #2256: write the namespace's `default_capabilities`
                // from the bundle if no governance op set it. Governance
                // ops are authoritative when present (they were applied
                // above); the bundle's value is a fallback for the case
                // where `create_group` populated defaults locally but
                // never published a `DefaultCapabilitiesSet` op (today's
                // codebase). Doing this *before* `add_group_member`
                // ensures the joiner's individual capability inherits
                // the right default — and reflects any admin-issued
                // override that travelled in the bundle, closing the
                // race where a stale joiner-side hard-coded constant
                // could otherwise override admin intent.
                if group_store::get_default_capabilities(&datastore, &group_id)?.is_none() {
                    group_store::set_default_capabilities(
                        &datastore,
                        &group_id,
                        join_result.default_capabilities,
                    )?;
                }

                // Add the joiner as a direct member of the namespace. The
                // call reads `default_capabilities` from the local store
                // (just populated above) and assigns the bit set to the
                // new member. Idempotent on the *direct*-row check — the
                // inheritance-aware `check_group_membership` would
                // wrongly skip the add when the joiner already inherits
                // membership from a parent namespace, leaving them
                // without the direct row that subsequent direct lookups
                // (removal, capability writes, list_group_members) need.
                if !group_store::has_direct_group_member(&datastore, &group_id, &joiner_identity)? {
                    group_store::add_group_member(&datastore, &group_id, &joiner_identity, role)?;
                } else {
                    info!(
                        ?group_id,
                        %joiner_identity,
                        "group member already recorded locally, skipping add_group_member"
                    );
                }

                // The joiner needs the group key to decrypt subsequent
                // group ops. Two delivery paths converge here:
                //
                //   1. Direct stream (`request_namespace_join` above).
                //      Authoritative when the served peer holds the key.
                //   2. Gossip `KeyDelivery` from any admin who applies our
                //      `MemberJoined` op below — Phase 9.1 already targets
                //      the recipient via `required_signers` so the admin
                //      knows whether *we* acked.
                //
                // If path 1 didn't deliver a key, we fall through to path 2
                // here and block `join_group` until either a `KeyDelivery`
                // for our identity arrives or `key_delivery_fallback_wait`
                // elapses. Subscribing BEFORE publishing MemberJoined
                // closes the race where an admin could see our op,
                // immediately publish KeyDelivery, and have us miss it
                // before subscribing.
                let needs_key_wait =
                    group_store::load_current_group_key(&datastore, &group_id)?.is_none();
                let mut op_event_rx = if needs_key_wait {
                    Some(subscribe_op_events())
                } else {
                    None
                };

                // Publish MemberJoined so other namespace members learn
                // about us. Joiner can't ack their own op (they're not yet
                // a recognised member from the receiver's view until the
                // op applies), so `required_signers = None` here — any
                // ack from any current member is fine.
                let member_joined_op = NamespaceOp::Root(RootOp::MemberJoined {
                    member: joiner_identity,
                    signed_invitation: invitation.clone(),
                });
                match group_store::sign_and_publish_namespace_op(
                    &datastore,
                    &node_client,
                    &ack_router,
                    namespace_id,
                    &sk,
                    member_joined_op,
                    None,
                )
                .await
                {
                    Ok(_report) => {}
                    Err(e) => warn!(?e, "failed to publish MemberJoined (non-fatal)"),
                }

                if let Some(rx) = op_event_rx.as_mut() {
                    let deadline = Instant::now() + key_delivery_fallback_wait;
                    loop {
                        // Re-check the store on every iteration: the apply
                        // path emits the event AFTER `store_group_key`, so
                        // any successful unwrap is observable as an
                        // already-stored key by the time we get woken.
                        // This also catches races where an event was
                        // dropped via `RecvError::Lagged` before we got to
                        // it (broadcast channel is process-wide and other
                        // tasks may flood it).
                        //
                        // Soft-fail on transient store errors: a single
                        // failed read should not abort the join — the loop
                        // already retries on every event tick and the
                        // deadline branch handles permanent failure. This
                        // mirrors the `RecvError::Lagged` arm's "we'll
                        // observe it next tick" semantics.
                        match group_store::load_current_group_key(&datastore, &group_id) {
                            Ok(Some(_)) => {
                                info!(
                                    ?group_id,
                                    "group key acquired via gossip KeyDelivery fallback"
                                );
                                break;
                            }
                            Ok(None) => {}
                            Err(e) => {
                                warn!(
                                    ?group_id,
                                    ?e,
                                    "transient store error during KeyDelivery wait — retrying"
                                );
                            }
                        }
                        let now = Instant::now();
                        if now >= deadline {
                            // Phase 12 (#2237) deferred this from a typed
                            // `Err` to a `warn!` + Ok-no-key. Restoring the
                            // typed-error contract: callers must be able to
                            // distinguish "joined and ready" from "joined
                            // but unusable yet". Returning Err here means
                            // the admin endpoint surfaces a failure that
                            // clients can retry, instead of clients
                            // proceeding to write to a context whose
                            // group key has not yet arrived.
                            return Err(eyre::eyre!(
                                "KeyDelivery timed out for group {group_id:?}: \
                                 no group key arrived within {}s via the gossip fallback path; \
                                 join cannot proceed without a usable group key",
                                key_delivery_fallback_wait.as_secs()
                            ));
                        }
                        let remaining = deadline - now;
                        match tokio::time::timeout(remaining, rx.recv()).await {
                            Ok(Ok(OpEvent::GroupKeyDelivered {
                                group_id: g,
                                recipient,
                            })) if g == group_id.to_bytes() && recipient == joiner_identity => {
                                // Loop back to re-read the store — the
                                // emitter publishes AFTER store_group_key
                                // succeeded, so the next iteration's
                                // `load_current_group_key` will hit and
                                // break cleanly.
                                continue;
                            }
                            Ok(Ok(_)) => continue, // unrelated event
                            Ok(Err(RecvError::Lagged(_))) => {
                                // Broadcast capacity exceeded — relevant
                                // events may have been dropped. Re-check
                                // store; if the key arrived during the
                                // overflow window we'll observe it on the
                                // next iteration and break cleanly.
                                continue;
                            }
                            Ok(Err(RecvError::Closed)) => {
                                // The static `op_events::NOTIFIER` cannot
                                // be dropped at runtime today, so this
                                // branch is functionally unreachable. If
                                // a future refactor changes that, fall
                                // through to the deadline check rather
                                // than spinning on a permanently-closed
                                // channel.
                                break;
                            }
                            Err(_) => continue, // timeout slice — outer deadline check handles exit
                        }
                    }
                }

                // -------------------------------------------------------
                // Phase 3: Auto-join contexts from the response.
                // -------------------------------------------------------

                if let Some(ref alias_str) = group_alias {
                    group_store::set_group_alias(&datastore, &group_id, alias_str)?;
                }

                let contexts = &join_result.context_ids;

                if let Some(meta) = group_store::load_group_meta(&datastore, &group_id)? {
                    if meta.auto_join {
                        info!(
                            ?group_id,
                            context_count = contexts.len(),
                            "auto-join: contexts from direct join response"
                        );
                        // Batch all ContextIdentity writes in a single handle to
                        // avoid per-context mutex acquisition overhead.
                        {
                            let mut handle = datastore.handle();
                            for context_id in contexts {
                                let ci_key =
                                    key::ContextIdentity::new(*context_id, joiner_identity);
                                if !handle.has(&ci_key)? {
                                    handle.put(
                                        &ci_key,
                                        &calimero_store::types::ContextIdentity {
                                            private_key: Some(*sk),
                                            sender_key: None,
                                        },
                                    )?;
                                }
                            }
                        }

                        // Register the context-under-group mapping
                        // (`ContextGroupRef`) directly from the bundle's
                        // `context_ids`. The bundle's `governance_ops`
                        // normally include a `ContextRegistered` op
                        // that would write the same mapping on apply,
                        // but the op list can be an incomplete snapshot
                        // — missing the op leaves the mapping unwritten
                        // and `get_group_for_context` returns `None`.
                        // Idempotent with the governance-op path.
                        for context_id in contexts {
                            if let Err(err) = group_store::register_context_in_group(
                                &datastore, &group_id, context_id,
                            ) {
                                warn!(
                                    %context_id,
                                    ?err,
                                    "failed to register context under group during join"
                                );
                            }
                        }

                        for context_id in contexts {
                            let config = if !context_client.has_context(context_id)? {
                                let zero_app =
                                    calimero_primitives::application::ApplicationId::from(
                                        [0u8; 32],
                                    );
                                let app_id = join_result.application_id;
                                let resolved = if app_id != zero_app {
                                    Some(app_id)
                                } else {
                                    group_store::load_group_meta(&datastore, &group_id)?
                                        .map(|m| m.target_application_id)
                                        .filter(|id| *id != zero_app)
                                };
                                let svc_name =
                                    group_store::get_context_service_name(&datastore, context_id)?;
                                Some(ContextConfigParams {
                                    application_id: resolved,
                                    application_revision: 0,
                                    members_revision: 0,
                                    service_name: svc_name,
                                })
                            } else {
                                None
                            };

                            if let Err(e) = context_client
                                .sync_context_config(*context_id, config)
                                .await
                            {
                                warn!(%context_id, ?e, "failed to sync context config");
                            }
                            if let Err(e) = node_client.subscribe(context_id).await {
                                warn!(%context_id, ?e, "failed to subscribe to context");
                            }
                            if let Err(e) = node_client.sync(Some(context_id), None).await {
                                warn!(%context_id, ?e, "failed to trigger context sync");
                            }
                        }
                    }
                }

                if let Err(e) = node_client.sync(None, None).await {
                    warn!(?e, "failed to trigger global sync after join");
                }

                info!(
                    ?group_id,
                    namespace_id = %hex::encode(namespace_id),
                    %joiner_identity,
                    "member joined group via direct request-response"
                );

                Ok(JoinGroupResponse {
                    group_id,
                    member_identity: joiner_identity,
                    governance_op_bytes: vec![],
                })
            }
            .into_actor(self),
        )
    }
}
