//! Network event handlers
//!
//! **SRP Applied**: Event handling is split into focused modules:
//! - `subscriptions` - context/group topic subscribe lifecycle
//! - `heartbeat` - hash heartbeat divergence detection and sync trigger
//! - `specialized` - specialized node invitation protocol
//! - `namespace` - namespace governance and heartbeat handling
//! - this file - dispatch and lightweight blob/listen handlers

use actix::{AsyncContext, Handler, WrapFuture};
use calimero_network_primitives::messages::NetworkEvent;
use calimero_node_primitives::sync::BroadcastMessage;
use tracing::{debug, error, info, warn};

use crate::handlers::{state_delta, stream_opened};
use crate::NodeManager;

mod heartbeat;
mod namespace;
mod specialized;
mod subscriptions;

impl Handler<NetworkEvent> for NodeManager {
    type Result = <NetworkEvent as actix::Message>::Result;

    fn handle(&mut self, msg: NetworkEvent, ctx: &mut Self::Context) -> Self::Result {
        match msg {
            NetworkEvent::ListeningOn { address, .. } => {
                info!("Listening on: {}", address);
            }
            NetworkEvent::Subscribed { peer_id, topic } => {
                subscriptions::handle_subscribed(self, ctx, peer_id, topic);
            }
            NetworkEvent::Unsubscribed { peer_id, topic } => {
                subscriptions::handle_unsubscribed(peer_id, topic);
            }
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
                        key_id,
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
                                    key_id,
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
                        heartbeat::handle_hash_heartbeat(
                            self,
                            ctx,
                            source,
                            context_id,
                            their_root_hash,
                            their_dag_heads,
                        );
                    }
                    BroadcastMessage::SpecializedNodeDiscovery { nonce, node_type } => {
                        let specialized_message =
                            BroadcastMessage::SpecializedNodeDiscovery { nonce, node_type };
                        let _handled = specialized::handle_specialized_broadcast(
                            self,
                            ctx,
                            source,
                            &topic,
                            &specialized_message,
                        );
                    }
                    BroadcastMessage::TeeAttestationAnnounce {
                        quote_bytes,
                        public_key,
                        nonce,
                        node_type,
                    } => {
                        let specialized_message = BroadcastMessage::TeeAttestationAnnounce {
                            quote_bytes,
                            public_key,
                            nonce,
                            node_type,
                        };
                        let _handled = specialized::handle_specialized_broadcast(
                            self,
                            ctx,
                            source,
                            &topic,
                            &specialized_message,
                        );
                    }
                    BroadcastMessage::SpecializedNodeJoinConfirmation { nonce } => {
                        let specialized_message =
                            BroadcastMessage::SpecializedNodeJoinConfirmation { nonce };
                        let _handled = specialized::handle_specialized_broadcast(
                            self,
                            ctx,
                            source,
                            &topic,
                            &specialized_message,
                        );
                    }
                    BroadcastMessage::NamespaceGovernanceDelta {
                        namespace_id,
                        delta_id: _,
                        parent_ids: _,
                        payload,
                    } => {
                        namespace::handle_namespace_governance_delta(
                            self,
                            ctx,
                            source,
                            namespace_id,
                            payload,
                        );
                    }
                    BroadcastMessage::NamespaceStateHeartbeat {
                        namespace_id,
                        dag_heads: peer_heads,
                    } => {
                        namespace::handle_namespace_state_heartbeat(
                            self,
                            ctx,
                            source,
                            namespace_id,
                            peer_heads,
                        );
                    }
                    _ => {
                        debug!(?message, "Received unknown broadcast message type");
                    }
                }
            }
            NetworkEvent::StreamOpened {
                peer_id,
                stream,
                protocol,
            } => {
                stream_opened::handle_stream_opened(self, ctx, peer_id, stream, protocol);
            }
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

                let blobstore = self.managers.blobstore.clone();
                let blob_data = data.clone();

                let _ignored = ctx.spawn(
                    async move {
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
            }
            NetworkEvent::SpecializedNodeVerificationRequest {
                peer_id,
                request_id,
                request,
                channel,
            } => {
                let _handled = specialized::handle_specialized_network_event(
                    self,
                    ctx,
                    NetworkEvent::SpecializedNodeVerificationRequest {
                        peer_id,
                        request_id,
                        request,
                        channel,
                    },
                );
            }
            NetworkEvent::SpecializedNodeInvitationResponse {
                peer_id,
                request_id,
                response,
            } => {
                let _handled = specialized::handle_specialized_network_event(
                    self,
                    ctx,
                    NetworkEvent::SpecializedNodeInvitationResponse {
                        peer_id,
                        request_id,
                        response,
                    },
                );
            }
        }
    }
}
