use actix::{AsyncContext, WrapFuture};
use calimero_network_primitives::messages::NetworkEvent;
use calimero_network_primitives::specialized_node_invite::SpecializedNodeType;
use calimero_node_primitives::sync::BroadcastMessage;
use tracing::{debug, error, info, warn};

use crate::handlers::{specialized_node_invite, tee_attestation_admission};
use crate::run::NodeMode;
use crate::NodeManager;

pub(super) fn handle_specialized_broadcast(
    this: &mut NodeManager,
    ctx: &mut actix::Context<NodeManager>,
    source: libp2p::PeerId,
    topic: &libp2p::gossipsub::TopicHash,
    message: &BroadcastMessage<'_>,
) -> bool {
    match message {
        BroadcastMessage::SpecializedNodeDiscovery { nonce, node_type } => {
            let should_respond = matches!(
                (this.state.node_mode(), *node_type),
                (NodeMode::ReadOnly, SpecializedNodeType::ReadOnly)
            );

            if !should_respond {
                debug!(
                    %source,
                    nonce = %hex::encode(*nonce),
                    ?node_type,
                    node_mode = ?this.state.node_mode(),
                    "Ignoring specialized node discovery (not a matching specialized node)"
                );
                return true;
            }

            info!(
                %source,
                nonce = %hex::encode(*nonce),
                ?node_type,
                "Received specialized node discovery - responding as read-only node"
            );

            let network_client = this.managers.sync.network_client.clone();
            let context_client = this.clients.context.clone();
            let nonce = *nonce;
            let _ignored = ctx.spawn(
                async move {
                    match specialized_node_invite::handle_specialized_node_discovery(
                        nonce,
                        source,
                        &context_client,
                    ) {
                        Ok(request) => {
                            if let Err(err) = network_client
                                .send_specialized_node_verification_request(source, request)
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
                            debug!(
                                error = %err,
                                "Failed to handle specialized node discovery (not a TEE node?)"
                            );
                        }
                    }
                }
                .into_actor(this),
            );
            true
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
                        warn!(
                            %source,
                            topic = %topic_str,
                            "Invalid group topic hex in TeeAttestationAnnounce"
                        );
                        return true;
                    }
                    bytes
                }
                None => {
                    warn!(
                        %source,
                        topic = %topic_str,
                        "TeeAttestationAnnounce received on non-group topic"
                    );
                    return true;
                }
            };

            info!(
                %source,
                %public_key,
                nonce = %hex::encode(*nonce),
                group_id = %hex::encode(group_id_bytes),
                "Received TEE attestation announce on group topic"
            );

            let context_client = this.clients.context.clone();
            let quote_bytes = quote_bytes.clone();
            let public_key = *public_key;
            let nonce = *nonce;
            let _ignored = ctx.spawn(
                async move {
                    if let Err(err) = tee_attestation_admission::handle_tee_attestation_announce(
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
                .into_actor(this),
            );
            true
        }
        BroadcastMessage::SpecializedNodeJoinConfirmation { nonce } => {
            info!(
                %source,
                nonce = %hex::encode(*nonce),
                "Received specialized node join confirmation"
            );

            let pending_invites = this.state.pending_specialized_node_invites_handle();
            specialized_node_invite::handle_join_confirmation(&pending_invites, *nonce);
            true
        }
        _ => false,
    }
}

pub(super) fn handle_specialized_network_event(
    this: &mut NodeManager,
    ctx: &mut actix::Context<NodeManager>,
    msg: NetworkEvent,
) -> bool {
    match msg {
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

            let pending_invites = this.state.pending_specialized_node_invites_handle();
            let network_client = this.managers.sync.network_client.clone();
            let context_client = this.clients.context.clone();
            let accept_mock_tee = this.state.accept_mock_tee();
            let _ignored = ctx.spawn(
                async move {
                    let response = specialized_node_invite::handle_verification_request(
                        peer_id,
                        request,
                        &pending_invites,
                        &context_client,
                        accept_mock_tee,
                    )
                    .await;

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
                .into_actor(this),
            );
            true
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

            let context_client = this.clients.context.clone();
            let network_client = this.managers.sync.network_client.clone();
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
                            info!(
                                %peer_id,
                                %context_id,
                                nonce = %hex::encode(nonce),
                                "Joined context, broadcasting join confirmation"
                            );

                            let payload =
                                BroadcastMessage::SpecializedNodeJoinConfirmation { nonce };
                            if let Ok(payload_bytes) = borsh::to_vec(&payload) {
                                let topic = libp2p::gossipsub::TopicHash::from_raw(context_id);
                                if let Err(err) = network_client.publish(topic, payload_bytes).await
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
                .into_actor(this),
            );
            true
        }
        _ => false,
    }
}
