//! Responder side of a forwarded outbound request (`SealedOutboundRequest`):
//! a member's node asks a TEE of the namespace to open a `{{sealed:…}}` secret
//! and send the request it names. The context manager decides and answers; see
//! `calimero_context::outbound_forward`.

use calimero_context_client::messages::ForwardedOutboundRequest;
use calimero_crypto::Nonce;
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::{MessagePayload, StreamMessage};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use tracing::debug;

use super::SyncManager;

impl SyncManager {
    /// Hands the request to the context manager and sends back its answer.
    /// `requester` has proven its key on this stream and is a member of
    /// `context_id`; `responder` is this node's identity there.
    pub(super) async fn handle_sealed_outbound_request(
        &self,
        context_id: ContextId,
        requester: PublicKey,
        responder: PublicKey,
        request: Vec<u8>,
        stream: &mut Stream,
        nonce: Nonce,
    ) -> eyre::Result<()> {
        let answer = self
            .context_client
            .forwarded_outbound(ForwardedOutboundRequest {
                context: context_id,
                requester,
                responder,
                request,
            })
            .await?;
        debug!(%context_id, %requester, "answering a forwarded outbound request");
        let answer = StreamMessage::Message {
            sequence_id: 0,
            payload: MessagePayload::SealedOutboundAnswer { answer },
            next_nonce: nonce,
        };
        crate::sync::stream::send(stream, &answer, None).await
    }
}
