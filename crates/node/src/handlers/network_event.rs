use std::num::NonZeroUsize;

use actix::{AsyncContext, Handler, Message, WrapFuture};
use calimero_context_primitives::client::ContextClient;
use calimero_crypto::{Nonce, SharedKey};
use calimero_network_primitives::messages::NetworkEvent;
use calimero_node_primitives::sync::BroadcastMessage;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use eyre::bail;
use libp2p::PeerId;
use owo_colors::OwoColorize;
use tracing::{debug, info, warn};

use crate::sync::SyncManager;
use crate::utils::choose_stream;
use crate::NodeManager;

impl Handler<NetworkEvent> for NodeManager {
    type Result = <NetworkEvent as Message>::Result;

    fn handle(&mut self, msg: NetworkEvent, ctx: &mut Self::Context) -> Self::Result {
        match msg {
            NetworkEvent::ListeningOn { address, .. } => info!("Listening on: {}", address),
            NetworkEvent::Subscribed {
                peer_id: their_peer_id,
                topic,
            } => {
                let Ok(context_id) = topic.as_str().parse() else {
                    return;
                };

                if !self
                    .context_client
                    .has_context(&context_id)
                    .unwrap_or_default()
                {
                    debug!(
                        %context_id,
                        %their_peer_id,
                        "Observed subscription to unknown context, ignoring.."
                    );

                    return;
                }

                info!(
                    "Peer '{}' subscribed to context '{}'",
                    their_peer_id.cyan(),
                    context_id.cyan()
                );
            }
            NetworkEvent::Unsubscribed {
                peer_id: their_peer_id,
                topic,
            } => {
                let Ok(context_id) = topic.as_str().parse() else {
                    return;
                };

                if !self
                    .context_client
                    .has_context(&context_id)
                    .unwrap_or_default()
                {
                    debug!(
                        %context_id,
                        %their_peer_id,
                        "Observed unsubscription to unknown context, ignoring.."
                    );

                    return;
                }

                info!(
                    "Peer '{}' unsubscribed to context '{}'",
                    their_peer_id, context_id
                );
            }
            NetworkEvent::Message { message, .. } => {
                let Some(source) = message.source else {
                    warn!(?message, "Received message without source");
                    return;
                };

                let message = match borsh::from_slice(&message.data) {
                    Ok(message) => message,
                    Err(err) => {
                        debug!(?err, ?message, "Failed to deserialize message");
                        return;
                    }
                };

                match message {
                    BroadcastMessage::StateDelta {
                        context_id,
                        author_id,
                        root_hash,
                        artifact,
                        height,
                        nonce,
                    } => {
                        let context_client = self.context_client.clone();
                        let sync_manager = self.sync_manager.clone();

                        let _ignored = ctx.spawn(
                            async move {
                                if let Err(err) = handle_state_delta(
                                    context_client,
                                    sync_manager,
                                    source,
                                    context_id,
                                    author_id,
                                    root_hash,
                                    artifact.into_owned(),
                                    height,
                                    nonce,
                                )
                                .await
                                {
                                    warn!(?err, "Failed to handle state delta");
                                }
                            }
                            .into_actor(self),
                        );
                    }
                    _ => {
                        debug!(?message, "Received unexpected message");
                    }
                }
            }
            NetworkEvent::StreamOpened { peer_id, stream } => {
                debug!(%peer_id, "Handling opened stream");

                let sync_manager = self.sync_manager.clone();

                let _ignored = ctx.spawn(
                    async move {
                        sync_manager.handle_opened_stream(stream).await;

                        debug!(%peer_id, "Handled opened stream");
                    }
                    .into_actor(self),
                );
            }
        }
    }
}

async fn handle_state_delta(
    context_client: ContextClient,
    sync_manager: SyncManager,
    source: PeerId,
    context_id: ContextId,
    author_id: PublicKey,
    root_hash: Hash,
    artifact: Vec<u8>,
    height: NonZeroUsize,
    nonce: Nonce,
) -> eyre::Result<()> {
    let Some(context) = context_client.get_context(&context_id)? else {
        bail!("context '{}' not found", context_id);
    };

    debug!(
        %context_id, %author_id,
        expected_root_hash = %root_hash,
        current_root_hash = %context.root_hash,
        "Received state delta"
    );

    if root_hash == context.root_hash {
        debug!(%context_id, "Received state delta with same root hash, ignoring..");
        return Ok(());
    }

    if let Some(known_height) = context_client.get_delta_height(&context_id, &author_id)? {
        if known_height >= height || height.get() - known_height.get() > 1 {
            debug!(%author_id, %context_id, "Received state delta much further ahead than known height, syncing..");

            let _ignored = sync_manager.initiate_sync(context_id, source).await;
            return Ok(());
        }
    }

    let Some(sender_key) = context_client
        .get_identity(&context_id, &author_id)?
        .and_then(|i| i.sender_key)
    else {
        debug!(%author_id, %context_id, "Missing sender key, initiating sync");

        let _ignored = sync_manager.initiate_sync(context_id, source).await;
        return Ok(());
    };

    let shared_key = SharedKey::from_sk(&sender_key);

    let Some(artifact) = shared_key.decrypt(artifact, nonce) else {
        debug!(%author_id, %context_id, "State delta decryption failed, initiating sync");

        let _ignored = sync_manager.initiate_sync(context_id, source).await;
        return Ok(());
    };

    let identities = context_client.context_members(&context_id, Some(true));

    let Some((our_identity, _)) = choose_stream(identities, &mut rand::thread_rng())
        .await
        .transpose()?
    else {
        bail!("no owned identities found for context: {}", context_id);
    };

    context_client.put_state_delta(&context_id, &author_id, &height, &artifact)?;

    let outcome = context_client
        .execute(
            &context_id,
            &our_identity,
            "__calimero_sync_next".to_owned(),
            artifact,
            vec![],
            None,
        )
        .await?;

    context_client.set_delta_height(&context_id, &author_id, height)?;

    if outcome.root_hash != root_hash {
        debug!(
            %context_id,
            %author_id,
            expected_root_hash = %root_hash,
            current_root_hash = %outcome.root_hash,
            "State delta application led to root hash mismatch, initiating sync"
        );

        let _ignored = sync_manager.initiate_sync(context_id, source).await;
        return Ok(());
    }

    Ok(())
}
