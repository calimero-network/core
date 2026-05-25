use actix::{AsyncContext, WrapFuture};
use calimero_network_primitives::messages::NetworkEvent;
use calimero_network_primitives::specialized_node_invite::SpecializedNodeType;
use calimero_node_primitives::sync::BroadcastMessage;
use tracing::{debug, error, info, warn};

use crate::handlers::{specialized_node_invite, tee_attestation_admission};
use crate::run::NodeMode;
use crate::NodeManager;

/// Why a gossipsub topic was rejected as a `TeeAttestationAnnounce`
/// namespace-governance topic.
#[derive(Debug, PartialEq, Eq)]
enum NamespaceTopicError {
    /// Topic did not carry the `ns/` namespace-governance prefix. Fleet
    /// TEE nodes publish on `ns/<hex(namespace_id)>` via
    /// `NodeClient::publish_on_namespace`, so anything else is not an
    /// admission announce.
    NotNamespaceTopic,
    /// Topic had the `ns/` prefix but the suffix was not a 32-byte hex id.
    MalformedHex,
}

/// Parse a `TeeAttestationAnnounce` gossipsub topic into its namespace id.
///
/// Fleet TEE nodes announce on `ns/<hex(namespace_id)>` (the namespace
/// governance topic — see `NodeClient::publish_on_namespace`,
/// `governance_broadcast::ns_topic`, and the `ns/` handling in
/// `subscriptions.rs`). The namespace IS its root group, so the returned
/// 32-byte id is used directly as the admission group id.
fn parse_namespace_announce_topic(topic_str: &str) -> Result<[u8; 32], NamespaceTopicError> {
    let hex = topic_str
        .strip_prefix("ns/")
        .ok_or(NamespaceTopicError::NotNamespaceTopic)?;
    let mut bytes = [0u8; 32];
    hex::decode_to_slice(hex, &mut bytes).map_err(|_| NamespaceTopicError::MalformedHex)?;
    Ok(bytes)
}

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
            // Fleet TEE nodes announce on the namespace governance topic
            // `ns/<hex(namespace_id)>` (see `NodeClient::publish_on_namespace`
            // and the `ns/` convention in `subscriptions.rs` /
            // `governance_broadcast::ns_topic`). The namespace IS its root
            // group, so the parsed namespace id is the admission group id.
            let namespace_id_bytes = match parse_namespace_announce_topic(topic_str) {
                Ok(bytes) => bytes,
                Err(NamespaceTopicError::MalformedHex) => {
                    warn!(
                        %source,
                        topic = %topic_str,
                        "Invalid namespace topic hex in TeeAttestationAnnounce"
                    );
                    return true;
                }
                Err(NamespaceTopicError::NotNamespaceTopic) => {
                    warn!(
                        %source,
                        topic = %topic_str,
                        "TeeAttestationAnnounce received on non-namespace topic"
                    );
                    return true;
                }
            };

            info!(
                %source,
                %public_key,
                nonce = %hex::encode(*nonce),
                namespace_id = %hex::encode(namespace_id_bytes),
                "Received TEE attestation announce on namespace topic"
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
                        namespace_id_bytes,
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

#[cfg(test)]
mod tests {
    use super::{parse_namespace_announce_topic, NamespaceTopicError};

    /// Regression test for the `ns/` vs `group/` topic mismatch (PR #2096):
    /// fleet TEE nodes announce `TeeAttestationAnnounce` on
    /// `ns/<hex(namespace_id)>`, but the dispatcher used to strip
    /// `group/`, so the announce fell into the "non-namespace topic" arm
    /// and was dropped — `handle_tee_attestation_announce` / `admit_tee_node`
    /// never ran, and fleet TEE nodes were never admitted to the namespace
    /// group. The dispatcher must resolve an `ns/` topic to its namespace id
    /// and route it into the admission path.
    #[test]
    fn ns_announce_topic_resolves_to_namespace_id_for_admission() {
        let namespace_id = [0x42u8; 32];
        let topic = format!("ns/{}", hex::encode(namespace_id));

        let parsed = parse_namespace_announce_topic(&topic)
            .expect("ns/<hex> announce topic must route into the admission path, not be dropped");

        // The resolved id is what gets handed to
        // `handle_tee_attestation_announce` → `admit_tee_node` as the
        // admission group id (the namespace is its own root group).
        assert_eq!(parsed, namespace_id);
    }

    /// The old (buggy) `group/<hex>` topic must NOT match this path anymore.
    /// `group/` is not how TEE announces are published (publish uses
    /// `publish_on_namespace` → `ns/`), so a `group/` topic here is a
    /// non-namespace topic and is correctly rejected rather than admitted.
    #[test]
    fn legacy_group_topic_is_not_a_namespace_announce_topic() {
        let topic = format!("group/{}", hex::encode([0x42u8; 32]));
        assert_eq!(
            parse_namespace_announce_topic(&topic),
            Err(NamespaceTopicError::NotNamespaceTopic),
        );
    }

    /// A non-prefixed topic (e.g. a raw context id) is not a namespace
    /// announce topic.
    #[test]
    fn unprefixed_topic_is_not_a_namespace_announce_topic() {
        assert_eq!(
            parse_namespace_announce_topic("some-context-id"),
            Err(NamespaceTopicError::NotNamespaceTopic),
        );
    }

    /// An `ns/` topic with a malformed (non-hex / wrong-length) suffix is
    /// reported distinctly so the dispatcher can warn precisely instead of
    /// silently treating it as the wrong kind of topic.
    #[test]
    fn ns_topic_with_malformed_hex_is_rejected_as_malformed() {
        assert_eq!(
            parse_namespace_announce_topic("ns/not-hex"),
            Err(NamespaceTopicError::MalformedHex),
        );
        // Right prefix, valid hex, wrong length (16 bytes, not 32).
        assert_eq!(
            parse_namespace_announce_topic(&format!("ns/{}", hex::encode([0u8; 16]))),
            Err(NamespaceTopicError::MalformedHex),
        );
    }
}
