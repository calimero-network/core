//! Direct TEE admission: a fleet node asks named peers to admit it.
//!
//! Both halves of one exchange. The initiator is `fleet-join`, which used to
//! have only the broadcast: publish `TeeAttestationAnnounce` on the namespace
//! topic and hope that a peer allowed to vouch — an admin, or an admitted TEE —
//! is in the gossip mesh to hear it. When the only such peer is an owner's
//! laptop behind a relay, that mesh forms intermittently or not at all, and a
//! miss is silent: the announcer just times out as "announced, not admitted".
//!
//! This is the same move invitations already made. A joiner is handed its
//! admitters' addresses, dials them, and gets a definite answer over a stream.
//! Here the addresses come from whoever assigned the node (mdma), and the
//! answer is the same verdict the broadcast receiver reaches.
//!
//! # Why the addresses carry no authority
//!
//! The responder runs [`verify_and_admit`] — the very function the broadcast
//! receiver runs — so the quote, the nonce binding, the credential, the
//! namespace's admission policy and the vouching rule are all decided exactly as
//! before. A peer that is not allowed to vouch says so, and the initiator moves
//! on. An address pointing somewhere hostile costs a dial and a refusal; it
//! cannot produce an admission, because every peer re-checks the voucher when
//! it applies the op.
//!
//! # Why the broadcast stays
//!
//! Older responders cannot decode this request and drop the stream, and a node
//! started without addresses has nobody to ask. `fleet-join` still announces in
//! both cases; this path only removes the dependence on the mesh when it can.
//!
//! [`verify_and_admit`]: crate::handlers::tee_attestation_admission::verify_and_admit

use calimero_crypto::Nonce;
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::client::TeeAdmissionParams;
use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use libp2p::PeerId;
use rand::RngExt;
use tracing::{debug, info, warn};

use super::SyncManager;

/// How many distinct admitter machines one request will try.
///
/// Same bound, for the same reason, as the invitation path: each unreachable
/// machine costs a full stream-open timeout, and `fleet-join` is itself bounded.
/// A namespace's admins and admitted TEEs are a handful, not dozens.
const MAX_ADMITTER_MACHINES: usize = 8;

impl SyncManager {
    /// Initiator side: ask each address in turn until one peer admits this node.
    ///
    /// Returns the peer that admitted it. An `Err` names every refusal, so a
    /// fleet operator reading the log sees "not a voucher" or "RTMR3 not in
    /// policy allowlist" instead of a bare timeout.
    pub(super) async fn initiate_tee_admission(
        &self,
        params: TeeAdmissionParams,
    ) -> eyre::Result<PeerId> {
        let TeeAdmissionParams {
            namespace_id,
            admitter_addrs,
            public_key,
            quote_bytes,
            nonce,
            account,
        } = params;

        let routes =
            super::namespace_join::group_admitter_routes(&admitter_addrs, MAX_ADMITTER_MACHINES);
        if routes.is_empty() {
            eyre::bail!(
                "no dialable admitter address for namespace {} (need multiaddrs ending in \
                 /p2p/<peer id>)",
                hex::encode(namespace_id)
            );
        }

        let pop = self.build_join_init_pop(namespace_id, public_key).await;

        let mut refusals: Vec<String> = Vec::new();
        for (peer, peer_routes) in routes {
            // Dial first: the peer is named precisely because the mesh may not
            // have connected us, and `open_stream` needs a connection.
            super::namespace_join::dial_admitter_machines(
                vec![(peer, peer_routes)],
                self.sync_config.open_stream_timeout,
                |addr| async move { self.network_client.dial(addr).await },
            )
            .await;

            match self
                .ask_one_for_tee_admission(
                    peer,
                    namespace_id,
                    public_key,
                    &quote_bytes,
                    nonce,
                    &account,
                    pop,
                )
                .await
            {
                Ok(()) => {
                    info!(
                        %peer,
                        namespace_id = %hex::encode(namespace_id),
                        "admitted to the namespace by a directly-asked admitter"
                    );
                    return Ok(peer);
                }
                Err(reason) => {
                    debug!(%peer, %reason, "direct TEE admission: peer did not admit us");
                    refusals.push(format!("{peer}: {reason}"));
                }
            }
        }

        eyre::bail!(
            "no admitter admitted this node to namespace {}: [{}]",
            hex::encode(namespace_id),
            refusals.join("; ")
        )
    }

    /// One request to one peer. `Err` carries the peer's reason, or what went
    /// wrong reaching it.
    #[allow(
        clippy::too_many_arguments,
        reason = "the fields of one request, borrowed rather than rebuilt per peer"
    )]
    async fn ask_one_for_tee_admission(
        &self,
        peer: PeerId,
        namespace_id: [u8; 32],
        public_key: PublicKey,
        quote_bytes: &[u8],
        nonce: [u8; 32],
        account: &calimero_governance_types::JoinAccountCredential,
        pop: Option<calimero_node_primitives::sync::InitProof>,
    ) -> Result<(), String> {
        let mut stream = self
            .sync_network
            .open_stream(peer)
            .await
            .map_err(|e| format!("could not open a stream: {e}"))?;

        let msg = StreamMessage::Init {
            // Namespace-scoped requests carry a sentinel context id; the proof
            // is bound to the namespace instead (see `build_join_init_pop`).
            context_id: ContextId::from([0u8; 32]),
            party_id: public_key,
            payload: InitPayload::TeeAdmissionRequest {
                namespace_id,
                quote_bytes: quote_bytes.to_vec(),
                public_key,
                nonce,
                account: Box::new(account.clone()),
            },
            next_nonce: rand::rng().random(),
            pop,
        };
        crate::sync::stream::send(&mut stream, &msg, None)
            .await
            .map_err(|e| format!("send failed: {e}"))?;

        match crate::sync::stream::recv(&mut stream, None, self.sync_config.timeout).await {
            Ok(Some(StreamMessage::Message {
                payload: MessagePayload::TeeAdmissionResponse { admitted, reason },
                ..
            })) => {
                if admitted {
                    Ok(())
                } else {
                    Err(reason)
                }
            }
            // An older responder cannot decode the request and drops the
            // stream; so does one that refused our proof of possession.
            Ok(other) => Err(format!(
                "unexpected answer {:?}; the peer may predate direct admission",
                other.as_ref().map(std::mem::discriminant)
            )),
            Err(e) => Err(format!("no answer: {e}")),
        }
    }

    /// Responder side: verify the attestation and admit, exactly as the
    /// broadcast receiver would, then say what happened.
    #[allow(
        clippy::too_many_arguments,
        reason = "the fields of the request, handed over as the dispatcher destructured them"
    )]
    pub(super) async fn handle_tee_admission_request(
        &self,
        peer_id: PeerId,
        namespace_id: [u8; 32],
        quote_bytes: Vec<u8>,
        public_key: PublicKey,
        attestation_nonce: [u8; 32],
        account: Box<calimero_governance_types::JoinAccountCredential>,
        stream: &mut Stream,
        nonce: Nonce,
    ) -> eyre::Result<()> {
        let (admitted, reason) = match crate::handlers::tee_attestation_admission::verify_and_admit(
            &self.context_client,
            peer_id,
            quote_bytes,
            public_key,
            attestation_nonce,
            namespace_id,
            account,
        )
        .await
        {
            Ok(verdict) => (verdict.admitted(), verdict.reason()),
            // A policy refusal or a fault. Its text is what the requester needs
            // — "RTMR3 not in policy allowlist" is an instruction to its owner.
            Err(err) => (false, format!("{err:#}")),
        };

        if admitted {
            info!(%peer_id, %public_key, namespace_id = %hex::encode(namespace_id), "admitted a TEE that asked directly");
        } else {
            warn!(%peer_id, %public_key, namespace_id = %hex::encode(namespace_id), %reason, "refused a direct TEE admission request");
        }

        let answer = StreamMessage::Message {
            sequence_id: 0,
            payload: MessagePayload::TeeAdmissionResponse { admitted, reason },
            next_nonce: nonce,
        };
        crate::sync::stream::send(stream, &answer, None).await?;
        Ok(())
    }
}
