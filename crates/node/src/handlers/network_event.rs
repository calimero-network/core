//! Network event handlers
//!
//! **SRP Applied**: Each event type is handled in its own focused module:
//! - `state_delta.rs` - BroadcastMessage::StateDelta processing
//! - `stream_opened.rs` - Stream routing (blob vs sync)
//! - `blob_protocol.rs` - Blob protocol implementation
//! - `specialized_node_invite.rs` - Specialized node invitation protocol
//! - This file - Simple event handlers (subscriptions, blobs, listening)

use crate::handlers::{specialized_node_invite, state_delta, stream_opened};
use crate::run::NodeMode;

use actix::{AsyncContext, Handler, WrapFuture};
use calimero_network_primitives::messages::NetworkEvent;
use calimero_network_primitives::specialized_node_invite::SpecializedNodeType;
use calimero_node_primitives::sync::BroadcastMessage;
use calimero_primitives::context::ContextId;
use tracing::{debug, error, info, warn};

use crate::NodeManager;

impl Handler<NetworkEvent> for NodeManager {
    type Result = <NetworkEvent as actix::Message>::Result;

    fn handle(&mut self, msg: NetworkEvent, ctx: &mut Self::Context) -> Self::Result {
        match msg {
            // Simple events - just logging
            NetworkEvent::ListeningOn { address, .. } => {
                info!("Listening on: {}", address);
            }

            NetworkEvent::Subscribed { peer_id, topic } => {
                let topic_str = topic.as_str();

                // Check for group topic: "group/<hex32>"
                if let Some(hex) = topic_str.strip_prefix("group/") {
                    let mut bytes = [0u8; 32];
                    if hex::decode_to_slice(hex, &mut bytes).is_ok() {
                        info!(%peer_id, group_id=%hex, "Peer subscribed to group topic, triggering sync");
                        let context_client = self.clients.context.clone();
                        let _ignored = ctx.spawn(
                            async move {
                                use calimero_context_config::types::ContextGroupId;
                                use calimero_context_primitives::group::SyncGroupRequest;

                                let group_id = ContextGroupId::from(bytes);
                                if let Err(err) = context_client
                                    .sync_group(SyncGroupRequest {
                                        group_id,
                                        requester: None,
                                    })
                                    .await
                                {
                                    warn!(
                                        ?err,
                                        "Failed to auto-sync group after peer subscription"
                                    );
                                }
                            }
                            .into_actor(self),
                        );

                        let context_client_alias = self.clients.context.clone();
                        let _ignored_alias = ctx.spawn(
                            async move {
                                use calimero_context_config::types::ContextGroupId;
                                use calimero_context_primitives::group::BroadcastGroupAliasesRequest;

                                let group_id = ContextGroupId::from(bytes);
                                if let Err(err) = context_client_alias
                                    .broadcast_group_aliases(BroadcastGroupAliasesRequest {
                                        group_id,
                                    })
                                    .await
                                {
                                    warn!(
                                        ?err,
                                        "Failed to re-broadcast group aliases after peer subscription"
                                    );
                                }
                            }
                            .into_actor(self),
                        );

                        let context_client_local_state = self.clients.context.clone();
                        let _ignored_local_state = ctx.spawn(
                            async move {
                                use calimero_context_config::types::ContextGroupId;
                                use calimero_context_primitives::group::BroadcastGroupLocalStateRequest;

                                let group_id = ContextGroupId::from(bytes);
                                if let Err(err) = context_client_local_state
                                    .broadcast_group_local_state(BroadcastGroupLocalStateRequest {
                                        group_id,
                                    })
                                    .await
                                {
                                    warn!(
                                        ?err,
                                        "Failed to re-broadcast group local state after peer subscription"
                                    );
                                }
                            }
                            .into_actor(self),
                        );
                    }
                    return;
                }

                let Ok(context_id): Result<ContextId, _> = topic_str.parse() else {
                    return;
                };

                if !self
                    .clients
                    .context
                    .has_context(&context_id)
                    .unwrap_or_default()
                {
                    debug!(
                        %context_id,
                        %peer_id,
                        "Observed subscription to unknown context, ignoring.."
                    );
                    return;
                }

                info!("Peer '{}' subscribed to context '{}'", peer_id, context_id);
            }

            NetworkEvent::Unsubscribed { peer_id, topic } => {
                let Ok(context_id): Result<ContextId, _> = topic.as_str().parse() else {
                    return;
                };

                info!(
                    "Peer '{}' unsubscribed from context '{}'",
                    peer_id, context_id
                );
            }

            // BroadcastMessage handling - delegate to state_delta module
            NetworkEvent::Message {
                message: gossip_message,
                ..
            } => {
                let topic = gossip_message.topic.clone();
                let Some(source) = gossip_message.source else {
                    warn!(?gossip_message, "Received message without source");
                    return;
                };

                let message = match borsh::from_slice::<BroadcastMessage<'_>>(&gossip_message.data)
                {
                    Ok(message) => message,
                    Err(err) => {
                        debug!(?err, ?gossip_message, "Failed to deserialize message");
                        return;
                    }
                };

                #[expect(clippy::match_same_arms, reason = "clearer separation")]
                match message {
                    BroadcastMessage::StateDelta {
                        context_id,
                        author_id,
                        delta_id,
                        parent_ids,
                        hlc,
                        root_hash,
                        artifact,
                        nonce,
                        events,
                        governance_epoch,
                    } => {
                        info!(
                            %context_id,
                            %author_id,
                            delta_id = ?delta_id,
                            parent_count = parent_ids.len(),
                            has_events = events.is_some(),
                            governance_epoch_len = governance_epoch.len(),
                            "Matched StateDelta message"
                        );

                        // Clone the components we need
                        let node_clients = self.clients.clone();
                        let node_state = self.state.clone();
                        let network_client = self.managers.sync.network_client.clone();
                        let sync_config_timeout = self.managers.sync.sync_config.timeout;

                        let _ignored = ctx.spawn(
                            async move {
                                if let Err(err) = state_delta::handle_state_delta(
                                    node_clients,
                                    node_state,
                                    network_client,
                                    sync_config_timeout,
                                    source,
                                    context_id,
                                    author_id,
                                    delta_id,
                                    parent_ids,
                                    hlc,
                                    root_hash,
                                    artifact.into_owned(),
                                    nonce,
                                    events.map(|e| e.into_owned()),
                                    governance_epoch,
                                )
                                .await
                                {
                                    warn!(?err, "Failed to handle state delta");
                                }
                            }
                            .into_actor(self),
                        );
                    }
                    BroadcastMessage::HashHeartbeat {
                        context_id,
                        root_hash: their_root_hash,
                        dag_heads: their_dag_heads,
                    } => {
                        let context_client = self.clients.context.clone();

                        // Check for divergence
                        if let Ok(Some(our_context)) = context_client.get_context(&context_id) {
                            // Compare DAG heads
                            let our_heads_set: std::collections::HashSet<_> =
                                our_context.dag_heads.iter().collect();
                            let their_heads_set: std::collections::HashSet<_> =
                                their_dag_heads.iter().collect();

                            // If we have the same DAG heads but different root hashes, we diverged!
                            if our_heads_set == their_heads_set
                                && our_context.root_hash != their_root_hash
                            {
                                error!(
                                    %context_id,
                                    ?source,
                                    our_hash = ?our_context.root_hash,
                                    their_hash = ?their_root_hash,
                                    dag_heads = ?their_dag_heads,
                                    "DIVERGENCE DETECTED: Same DAG heads but different root hash!"
                                );

                                // Trigger sync to recover from divergence
                                // The periodic sync will eventually run state sync protocol
                                warn!(
                                    %context_id,
                                    ?source,
                                    their_heads = ?their_dag_heads,
                                    "Divergence detected - periodic sync will recover"
                                );
                            } else if our_context.root_hash != their_root_hash {
                                // Different root hash could mean:
                                // 1. We're behind (peer has more DAG heads than us)
                                // 2. Peer is behind (we have more DAG heads)
                                // 3. We forked (different DAG heads, both valid)

                                // Check if peer has DAG heads we don't have (we're behind)
                                let heads_we_dont_have: Vec<_> =
                                    their_heads_set.difference(&our_heads_set).collect();

                                if !heads_we_dont_have.is_empty() {
                                    info!(
                                        %context_id,
                                        ?source,
                                        our_heads_count = our_context.dag_heads.len(),
                                        their_heads_count = their_dag_heads.len(),
                                        missing_count = heads_we_dont_have.len(),
                                        "Peer has DAG heads we don't have - triggering sync"
                                    );

                                    // Trigger immediate sync to catch up
                                    let node_client = self.clients.node.clone();
                                    let ctx_spawn = ctx.spawn(async move {
                                        if let Err(e) = node_client.sync(Some(&context_id), None).await {
                                            warn!(%context_id, ?e, "Failed to trigger sync from heartbeat");
                                        }
                                    }.into_actor(self));
                                    let _ignored = ctx_spawn;
                                } else {
                                    debug!(
                                        %context_id,
                                        ?source,
                                        our_heads_count = our_context.dag_heads.len(),
                                        their_heads_count = their_dag_heads.len(),
                                        "Different root hash (peer is behind or concurrent updates)"
                                    );
                                }
                            }
                        }
                    }
                    BroadcastMessage::SpecializedNodeDiscovery { nonce, node_type } => {
                        // Only specialized nodes should respond to discovery broadcasts
                        // Check if this node's mode matches the requested node_type
                        let should_respond = match (self.state.node_mode, node_type) {
                            (NodeMode::ReadOnly, SpecializedNodeType::ReadOnly) => true,
                            _ => false,
                        };

                        if !should_respond {
                            debug!(
                                %source,
                                nonce = %hex::encode(nonce),
                                ?node_type,
                                node_mode = ?self.state.node_mode,
                                "Ignoring specialized node discovery (not a matching specialized node)"
                            );
                            return;
                        }

                        info!(
                            %source,
                            nonce = %hex::encode(nonce),
                            ?node_type,
                            "Received specialized node discovery - responding as read-only node"
                        );

                        let network_client = self.managers.sync.network_client.clone();
                        let context_client = self.clients.context.clone();

                        let _ignored = ctx.spawn(
                            async move {
                                // Generate verification request (includes identity creation)
                                match specialized_node_invite::handle_specialized_node_discovery(
                                    nonce,
                                    source,
                                    &context_client,
                                ) {
                                    Ok(request) => {
                                        // Send the verification request to the source peer
                                        if let Err(err) = network_client
                                            .send_specialized_node_verification_request(
                                                source, request,
                                            )
                                            .await
                                        {
                                            error!(
                                                %source,
                                                error = %err,
                                                "Failed to send specialized node verification request"
                                            );
                                        }
                                    }
                                    Err(err) => {
                                        // Verification generation failed (likely not on TEE hardware)
                                        debug!(
                                            error = %err,
                                            "Failed to handle specialized node discovery (not a TEE node?)"
                                        );
                                    }
                                }
                            }
                            .into_actor(self),
                        );
                    }
                    BroadcastMessage::TeeAttestationAnnounce {
                        quote_bytes,
                        public_key,
                        nonce,
                        node_type: _,
                    } => {
                        let topic_str = topic.as_str();
                        let group_id_bytes = match topic_str.strip_prefix("group/") {
                            Some(hex) => {
                                let mut bytes = [0u8; 32];
                                if hex::decode_to_slice(hex, &mut bytes).is_err() {
                                    warn!(%source, topic = %topic_str, "Invalid group topic hex in TeeAttestationAnnounce");
                                    return;
                                }
                                bytes
                            }
                            None => {
                                warn!(%source, topic = %topic_str, "TeeAttestationAnnounce received on non-group topic");
                                return;
                            }
                        };

                        info!(
                            %source,
                            %public_key,
                            nonce = %hex::encode(nonce),
                            group_id = %hex::encode(group_id_bytes),
                            "Received TEE attestation announce on group topic"
                        );

                        let context_client = self.clients.context.clone();

                        let _ignored = ctx.spawn(
                            async move {
                                if let Err(err) =
                                    super::tee_attestation_admission::handle_tee_attestation_announce(
                                        &context_client,
                                        source,
                                        quote_bytes,
                                        public_key,
                                        nonce,
                                        group_id_bytes,
                                    )
                                    .await
                                {
                                    warn!(
                                        %source,
                                        error = %err,
                                        "Failed to handle TEE attestation announce"
                                    );
                                }
                            }
                            .into_actor(self),
                        );
                    }
                    BroadcastMessage::SpecializedNodeJoinConfirmation { nonce } => {
                        // Standard nodes receive this confirmation on context topics
                        // when a specialized node successfully joins
                        info!(
                            %source,
                            nonce = %hex::encode(nonce),
                            "Received specialized node join confirmation"
                        );

                        // Handle the confirmation to remove the pending invite
                        let pending_invites = self.state.pending_specialized_node_invites.clone();
                        specialized_node_invite::handle_join_confirmation(&pending_invites, nonce);
                    }
                    BroadcastMessage::NamespaceGovernanceDelta {
                        namespace_id,
                        delta_id: _,
                        parent_ids: _,
                        payload,
                    } => {
                        use calimero_context_primitives::local_governance::SignedNamespaceOp;
                        use calimero_node_primitives::sync::MAX_SIGNED_GROUP_OP_PAYLOAD_BYTES;

                        if payload.len() > MAX_SIGNED_GROUP_OP_PAYLOAD_BYTES {
                            warn!(
                                len = payload.len(),
                                "oversized NamespaceGovernanceDelta payload"
                            );
                            return;
                        }

                        let op: SignedNamespaceOp = match borsh::from_slice(&payload) {
                            Ok(op) => op,
                            Err(err) => {
                                warn!(%err, "failed to decode NamespaceGovernanceDelta payload");
                                return;
                            }
                        };

                        if op.namespace_id != namespace_id {
                            warn!("NamespaceGovernanceDelta namespace_id mismatch with topic");
                            return;
                        }

                        if let Err(err) = op.verify_signature() {
                            warn!(%err, "NamespaceGovernanceDelta signature verification failed");
                            return;
                        }

                        let context_client = self.clients.context.clone();
                        let _ignored = ctx.spawn(
                            async move {
                                if let Err(err) =
                                    context_client.apply_signed_namespace_op(op.clone()).await
                                {
                                    warn!(?err, %source, "failed to apply namespace governance delta");
                                }
                            }
                            .into_actor(self),
                        );
                    }
                    BroadcastMessage::NamespaceStateHeartbeat {
                        namespace_id,
                        dag_heads: peer_heads,
                    } => {
                        let context_client = self.clients.context.clone();
                        let network_client = self.managers.sync.network_client.clone();
                        let sync_timeout = self.managers.sync.sync_config.timeout;

                        let _ignored = ctx.spawn(
                            async move {
                                let store =
                                    context_client.datastore_handle().into_inner();
                                let ns_head_key =
                                    calimero_store::key::NamespaceGovHead::new(namespace_id);
                                let handle = store.handle();
                                let local_heads: std::collections::HashSet<[u8; 32]> =
                                    match handle.get(&ns_head_key) {
                                        Ok(Some(h)) => h.dag_heads.into_iter().collect(),
                                        _ => std::collections::HashSet::new(),
                                    };
                                drop(handle);

                                // Check what peer has that we don't.
                                let we_need: Vec<[u8; 32]> = peer_heads
                                    .iter()
                                    .filter(|h| !local_heads.contains(*h))
                                    .copied()
                                    .collect();

                                // Check what we have that peer doesn't — if the
                                // peer sent empty heads (new joiner), re-publish
                                // all our namespace ops so they receive them.
                                let peer_head_set: std::collections::HashSet<[u8; 32]> =
                                    peer_heads.iter().copied().collect();
                                let peer_needs: Vec<[u8; 32]> = local_heads
                                    .iter()
                                    .filter(|h| !peer_head_set.contains(*h))
                                    .copied()
                                    .collect();

                                if !peer_needs.is_empty() {
                                    // Re-publish our namespace ops so the peer
                                    // (who may have just joined) receives them.
                                    let store_inner =
                                        context_client.datastore_handle().into_inner();
                                    let handle_inner = store_inner.handle();
                                    for delta_id in &peer_needs {
                                        let key = calimero_store::key::NamespaceGovOp::new(
                                            namespace_id,
                                            *delta_id,
                                        );
                                        if let Ok(Some(value)) = handle_inner.get(&key) {
                                            let payload =
                                                BroadcastMessage::NamespaceGovernanceDelta {
                                                    namespace_id,
                                                    delta_id: *delta_id,
                                                    parent_ids: vec![],
                                                    payload: value.skeleton_bytes,
                                                };
                                            if let Ok(bytes) = borsh::to_vec(&payload) {
                                                let topic = libp2p::gossipsub::TopicHash::from_raw(
                                                    format!("ns/{}", hex::encode(namespace_id)),
                                                );
                                                let _ = network_client
                                                    .publish(topic, bytes)
                                                    .await;
                                            }
                                        }
                                    }
                                }

                                if we_need.is_empty() {
                                    return;
                                }
                                let missing = we_need;

                                info!(
                                    namespace_id = %hex::encode(namespace_id),
                                    missing = missing.len(),
                                    %source,
                                    "namespace heartbeat divergence: requesting missing deltas"
                                );

                                let Ok(mut stream) =
                                    network_client.open_stream(source).await
                                else {
                                    debug!(
                                        %source,
                                        "failed to open stream for namespace delta catch-up"
                                    );
                                    return;
                                };

                                let msg =
                                    calimero_node_primitives::sync::StreamMessage::Init {
                                        context_id:
                                            calimero_primitives::context::ContextId::from(
                                                [0u8; 32],
                                            ),
                                        party_id:
                                            calimero_primitives::identity::PublicKey::from(
                                                [0u8; 32],
                                            ),
                                        payload: calimero_node_primitives::sync::InitPayload::NamespaceBackfillRequest {
                                            namespace_id,
                                            delta_ids: missing,
                                        },
                                        next_nonce: {
                                            use rand::Rng;
                                            rand::thread_rng().gen()
                                        },
                                    };

                                if let Err(err) =
                                    crate::sync::stream::send(&mut stream, &msg, None).await
                                {
                                    debug!(%err, "failed to send NamespaceBackfillRequest");
                                    return;
                                }

                                match crate::sync::stream::recv(
                                    &mut stream,
                                    None,
                                    sync_timeout,
                                )
                                .await
                                {
                                    Ok(Some(
                                        calimero_node_primitives::sync::StreamMessage::Message {
                                            payload:
                                                calimero_node_primitives::sync::MessagePayload::NamespaceBackfillResponse {
                                                    deltas,
                                                },
                                            ..
                                        },
                                    )) => {
                                        for (_delta_id, op_bytes) in deltas {
                                            if let Ok(op) = borsh::from_slice::<
                                                calimero_context_primitives::local_governance::SignedNamespaceOp,
                                            >(
                                                &op_bytes
                                            ) {
                                                let _ =
                                                    context_client
                                                        .apply_signed_namespace_op(op)
                                                        .await;
                                            }
                                        }
                                    }
                                    _ => {
                                        debug!(
                                            "unexpected response to NamespaceBackfillRequest"
                                        );
                                    }
                                }
                            }
                            .into_actor(self),
                        );
                    }
                    _ => {
                        // Future message types - log and ignore
                        debug!(?message, "Received unknown broadcast message type");
                    }
                }
            }

            // Stream routing - delegate to stream_opened module
            NetworkEvent::StreamOpened {
                peer_id,
                stream,
                protocol,
            } => {
                stream_opened::handle_stream_opened(self, ctx, peer_id, stream, protocol);
            }

            // Blob events - simple logging (applications can listen to these)
            NetworkEvent::BlobRequested {
                blob_id,
                context_id,
                requesting_peer,
            } => {
                debug!(
                    blob_id = %blob_id,
                    context_id = %context_id,
                    requesting_peer = %requesting_peer,
                    "Blob requested by peer"
                );
                // Applications can listen to this event for custom logic
            }

            NetworkEvent::BlobProvidersFound {
                blob_id,
                context_id,
                providers,
            } => {
                debug!(
                    blob_id = %blob_id,
                    context_id = ?context_id.as_ref().map(|id| id.to_string()),
                    providers_count = providers.len(),
                    "Blob providers found in DHT"
                );
                // Applications can listen to this event for custom logic
            }

            NetworkEvent::BlobDownloaded {
                blob_id,
                context_id,
                data,
                from_peer,
            } => {
                info!(
                    blob_id = %blob_id,
                    context_id = %context_id,
                    from_peer = %from_peer,
                    data_size = data.len(),
                    "Blob downloaded successfully from peer"
                );

                // Store the downloaded blob data to blobstore
                let blobstore = self.managers.blobstore.clone();
                let blob_data = data.clone();

                let _ignored = ctx.spawn(
                    async move {
                        // Convert data to async reader for blobstore.put()
                        let reader = &blob_data[..];

                        match blobstore.put(reader).await {
                            Ok((stored_blob_id, _hash, size)) => {
                                info!(
                                    requested_blob_id = %blob_id,
                                    stored_blob_id = %stored_blob_id,
                                    size = size,
                                    "Blob stored successfully"
                                );
                            }
                            Err(e) => {
                                error!(
                                    blob_id = %blob_id,
                                    error = %e,
                                    "Failed to store downloaded blob"
                                );
                            }
                        }
                    }
                    .into_actor(self),
                );
            }

            NetworkEvent::BlobDownloadFailed {
                blob_id,
                context_id,
                from_peer,
                error,
            } => {
                info!(
                    blob_id = %blob_id,
                    context_id = %context_id,
                    from_peer = %from_peer,
                    error = %error,
                    "Blob download failed"
                );
                // Applications can listen to this event for retry logic
            }

            // Specialized node invite protocol events
            NetworkEvent::SpecializedNodeVerificationRequest {
                peer_id,
                request_id,
                request,
                channel,
            } => {
                info!(
                    %peer_id,
                    ?request_id,
                    nonce = %hex::encode(request.nonce()),
                    public_key = %request.public_key(),
                    "Received specialized node verification request"
                );

                // Standard nodes verify and send invitation
                let pending_invites = self.state.pending_specialized_node_invites.clone();
                let network_client = self.managers.sync.network_client.clone();
                let context_client = self.clients.context.clone();
                let accept_mock_tee = self.state.accept_mock_tee;

                let _ignored = ctx.spawn(
                    async move {
                        // Verify and create invitation
                        let response = specialized_node_invite::handle_verification_request(
                            peer_id,
                            request,
                            &pending_invites,
                            &context_client,
                            accept_mock_tee,
                        )
                        .await;

                        // Send response back via the channel
                        if let Err(err) = network_client
                            .send_specialized_node_invitation_response(channel, response)
                            .await
                        {
                            error!(
                                %peer_id,
                                error = %err,
                                "Failed to send specialized node invitation response"
                            );
                        }
                    }
                    .into_actor(self),
                );
            }

            NetworkEvent::SpecializedNodeInvitationResponse {
                peer_id,
                request_id,
                response,
            } => {
                let nonce = response.nonce;
                info!(
                    %peer_id,
                    ?request_id,
                    nonce = %hex::encode(nonce),
                    has_invitation = response.invitation_bytes.is_some(),
                    has_error = response.error.is_some(),
                    "Received specialized node invitation response"
                );

                // Specialized nodes receive invitation and join context
                let context_client = self.clients.context.clone();
                let network_client = self.managers.sync.network_client.clone();

                let _ignored = ctx.spawn(
                    async move {
                        match specialized_node_invite::handle_specialized_node_invitation_response(
                            peer_id,
                            nonce,
                            response,
                            &context_client,
                        )
                        .await
                        {
                            Ok(Some(context_id)) => {
                                // Successfully joined - broadcast confirmation on context topic
                                info!(
                                    %peer_id,
                                    %context_id,
                                    nonce = %hex::encode(nonce),
                                    "Joined context, broadcasting join confirmation"
                                );

                                // Broadcast confirmation on the context topic
                                let payload =
                                    BroadcastMessage::SpecializedNodeJoinConfirmation { nonce };
                                if let Ok(payload_bytes) = borsh::to_vec(&payload) {
                                    let topic = libp2p::gossipsub::TopicHash::from_raw(context_id);
                                    if let Err(err) =
                                        network_client.publish(topic, payload_bytes).await
                                    {
                                        error!(
                                            %context_id,
                                            error = %err,
                                            "Failed to broadcast join confirmation"
                                        );
                                    }
                                }
                            }
                            Ok(None) => {
                                // Join failed or was rejected - no confirmation needed
                                debug!(
                                    %peer_id,
                                    nonce = %hex::encode(nonce),
                                    "Specialized node invitation response handled but join failed"
                                );
                            }
                            Err(err) => {
                                error!(
                                    %peer_id,
                                    error = %err,
                                    "Failed to handle specialized node invitation response"
                                );
                            }
                        }
                    }
                    .into_actor(self),
                );
            }
        }
    }
}
