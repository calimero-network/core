//! Network event handlers
//!
//! **SRP Applied**: Event handling is split into focused modules:
//! - `subscriptions` - context/group topic subscribe lifecycle
//! - `heartbeat` - hash heartbeat divergence detection and sync trigger
//! - `specialized` - specialized node invitation protocol
//! - `namespace` - namespace governance and heartbeat handling
//! - `blobs` - blob request/provider/download event handling
//! - this file - dispatch wiring only

use actix::Handler;
use calimero_network_primitives::messages::NetworkEvent;
use calimero_node_primitives::sync::BroadcastMessage;
use tracing::{debug, error, info, warn};

use crate::handlers::{state_delta, stream_opened};
use crate::state_delta_bridge::{StateDeltaJob, StateDeltaSendError};
use crate::NodeManager;

mod blobs;
mod heartbeat;
mod namespace;
mod readiness;
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
                subscriptions::handle_unsubscribed(self, peer_id, topic);
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

                match message {
                    BroadcastMessage::StateDelta {
                        context_id,
                        author_id,
                        delta_id,
                        parent_ids,
                        hlc,
                        artifact,
                        nonce,
                        governance_position,
                        key_id,
                        delta_signature,
                        producing_app_key,
                    } => {
                        info!(
                            %context_id,
                            %author_id,
                            delta_id = ?delta_id,
                            parent_count = parent_ids.len(),
                            governance_dag_heads_len = governance_position
                                .as_ref()
                                .map(|p| p.governance_dag_heads.len())
                                .unwrap_or(0),
                            "Matched StateDelta message"
                        );

                        let job = StateDeltaJob {
                            context: state_delta::StateDeltaContext {
                                node_clients: self.clients.clone(),
                                node_state: self.state.clone(),
                                network_client: self.managers.sync.network_client.clone(),
                                sync_timeout: self.managers.sync.sync_config.timeout,
                            },
                            message: state_delta::StateDeltaMessage {
                                source,
                                context_id,
                                author_id,
                                delta_id,
                                parent_ids,
                                hlc,
                                artifact: artifact.into_owned(),
                                nonce,
                                governance_position,
                                key_id,
                                delta_signature,
                                // Carry the sender-stamped producing_app_key
                                // through so the fence check (Tasks 8/9) can
                                // read it inside the apply path.
                                producing_app_key,
                            },
                        };

                        // Issue #2299: route StateDelta to its dedicated
                        // actor on a separate Arbiter. Drops on overflow
                        // are recovered by the existing heartbeat-driven
                        // rebroadcast path.
                        if let Err(err) = self.state_delta_tx.try_send(job) {
                            match err {
                                StateDeltaSendError::Full => {
                                    warn!(
                                        %context_id,
                                        %author_id,
                                        delta_id = ?delta_id,
                                        "StateDelta mailbox full — dropping; peer rebroadcast via heartbeat will retry"
                                    );
                                }
                                StateDeltaSendError::Closed => {
                                    error!(
                                        %context_id,
                                        %author_id,
                                        delta_id = ?delta_id,
                                        "StateDelta actor stopped — dispatch closed"
                                    );
                                }
                            }
                        }
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
                blobs::handle_blob_requested(blob_id, context_id, requesting_peer);
            }
            NetworkEvent::BlobProvidersFound {
                blob_id,
                context_id,
                providers,
            } => {
                blobs::handle_blob_providers_found(blob_id, context_id, providers);
            }
            NetworkEvent::BlobDownloaded {
                blob_id,
                context_id,
                data,
                from_peer,
            } => {
                blobs::handle_blob_downloaded(self, ctx, blob_id, context_id, data, from_peer);
            }
            NetworkEvent::BlobDownloadFailed {
                blob_id,
                context_id,
                from_peer,
                error,
            } => {
                blobs::handle_blob_download_failed(blob_id, context_id, from_peer, error);
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
