use calimero_context_primitives::client::ContextClient;
use calimero_network_primitives::client::NetworkClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use eyre::{bail, OptionExt, Result};
use libp2p::PeerId;
use tracing::{debug, info};

use crate::utils::choose_stream;

pub(super) async fn request_key_share_with_peer(
    network_client: &NetworkClient,
    context_client: &ContextClient,
    context_id: &ContextId,
    author_identity: &PublicKey,
    peer: PeerId,
    timeout: std::time::Duration,
) -> Result<()> {
    use calimero_crypto::{Nonce, SharedKey};
    use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};
    use rand::Rng;

    debug!(
        %context_id,
        %author_identity,
        %peer,
        "Initiating bidirectional key share with peer"
    );

    let timeout_result = tokio::time::timeout(timeout, async {
        let mut stream = network_client.open_stream(peer).await?;

        let identities = context_client.get_context_members(context_id, Some(true));
        let Some((our_identity, _)) = choose_stream(identities, &mut rand::thread_rng())
            .await
            .transpose()?
        else {
            bail!("no owned identities found for context: {}", context_id);
        };

        let our_nonce = rand::thread_rng().gen::<Nonce>();

        crate::sync::stream::send(
            &mut stream,
            &StreamMessage::Init {
                context_id: *context_id,
                party_id: our_identity,
                payload: InitPayload::KeyShare,
                next_nonce: our_nonce,
            },
            None,
        )
        .await?;

        let Some(ack) = crate::sync::stream::recv(&mut stream, None, timeout).await? else {
            bail!("connection closed while awaiting key share handshake");
        };

        let their_nonce = match ack {
            StreamMessage::Init {
                payload: InitPayload::KeyShare,
                next_nonce,
                ..
            } => next_nonce,
            unexpected => {
                bail!("unexpected message during key share: {:?}", unexpected)
            }
        };

        let mut their_identity = context_client
            .get_identity(context_id, author_identity)?
            .ok_or_eyre("expected peer identity to exist")?;

        let (private_key, sender_key) = context_client
            .get_identity(context_id, &our_identity)?
            .and_then(|i| Some((i.private_key?, i.sender_key?)))
            .ok_or_eyre("expected own identity to have private & sender keys")?;

        let shared_key = SharedKey::new(&private_key, &their_identity.public_key);

        crate::sync::stream::send(
            &mut stream,
            &StreamMessage::Message {
                sequence_id: 0,
                payload: MessagePayload::KeyShare { sender_key },
                next_nonce: our_nonce,
            },
            Some((shared_key, our_nonce)),
        )
        .await?;

        let Some(msg) =
            crate::sync::stream::recv(&mut stream, Some((shared_key, their_nonce)), timeout)
                .await?
        else {
            bail!("connection closed while awaiting sender_key");
        };

        let their_sender_key = match msg {
            StreamMessage::Message {
                payload: MessagePayload::KeyShare { sender_key },
                ..
            } => sender_key,
            unexpected => {
                bail!("unexpected message: {:?}", unexpected)
            }
        };

        their_identity.sender_key = Some(their_sender_key);
        context_client.update_identity(context_id, &their_identity)?;

        info!(
            %context_id,
            our_identity=%our_identity,
            their_identity=%author_identity,
            %peer,
            "Bidirectional key share completed"
        );

        Ok(())
    })
    .await
    .map_err(|_| eyre::eyre!("Timeout during key share with peer"))?;

    timeout_result?;

    Ok(())
}
