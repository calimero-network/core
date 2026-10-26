//! Outbound requests naming a `{{sealed:…}}` secret, from a node that cannot
//! open it to a TEE of the namespace that can.
//!
//! A sealed secret opens only on a TEE authority of the namespace, but a TEE
//! node's own runs are read-only: whatever a run on a TEE writes is discarded
//! unless the TEE scheduler fired it. So the run that needs the secret, and
//! stores what it learned, stays on the member's node, and only the HTTP
//! request travels:
//!
//! 1. The member's node checks the request against its own gates
//!    ([`OutboundService::check`](crate::outbound::OutboundService::check)),
//!    then sends it on the sync stream
//!    ([`InitPayload::SealedOutboundRequest`]) to a peer on the namespace topic,
//!    proving its context identity on that stream.
//! 2. The peer serves it only as a TEE authority of the namespace, and only
//!    for a member of the context that may write there
//!    ([`answer_forwarded`]). It runs every gate again under its own operator's
//!    policy, opens the secret, sends the request, redacts the secret from the
//!    response, and signs the response with its context identity.
//! 3. The member's node takes the response only under the signature of a TEE
//!    authority of the namespace ([`read_forwarded_answer`]); a peer that is not
//!    a TEE answers [`ForwardedAnswer::NotTee`] and the next one is asked.
//!
//! The secret itself never leaves the TEE, and a request only ever reaches the
//! hosts the secret was sealed for.

use std::sync::Arc;
use std::time::Duration;

use calimero_node_primitives::client::NodeClient;
use calimero_node_primitives::sync::{InitPayload, InitProof, MessagePayload, StreamMessage};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_primitives::outbound::{OutboundRefusal, TIMEOUT_MS};
use calimero_store::Store;
use eyre::{bail, eyre, Result as EyreResult};
use futures_util::{SinkExt, StreamExt};
use tracing::debug;

use crate::outbound::{
    forwarded_answer_message, read_forwarded_answer, ForwardedAnswer, Forwarder, OutboundService,
    SealedKeys,
};
use crate::scope_projection::{FoldedProjections, ScopeProjections};

/// How many namespace peers a request is offered to before it is refused.
const MAX_PEERS_ASKED: usize = 8;

/// How long a TEE may take to answer: its own request's limit, and some room
/// for the stream.
const ANSWER_TIMEOUT: Duration = Duration::from_millis(TIMEOUT_MS + 5_000);

/// How long opening a stream to one peer may take.
const OPEN_TIMEOUT: Duration = Duration::from_secs(5);

/// The [`Forwarder`] for a run in `context_id` as `identity`, which hands a
/// request to a TEE of the namespace over the network.
///
/// It is called on the run's blocking thread, so the exchange runs on the
/// node's runtime (`runtime`) and the thread waits for it.
pub(crate) fn network_forwarder(
    runtime: tokio::runtime::Handle,
    node_client: NodeClient,
    datastore: Store,
    scope_projections: Arc<std::sync::RwLock<ScopeProjections>>,
    context_id: ContextId,
    identity: PublicKey,
) -> Forwarder {
    Arc::new(move |request: &[u8]| {
        let unavailable = |why: &str| {
            format!(
                "{}: this node is not a TEE of the namespace, and {why}",
                OutboundRefusal::SealedUnavailable.word()
            )
        };
        let tee_keys = {
            let folded = FoldedProjections(&scope_projections);
            calimero_governance_store::tee_authority_keys_for_context(
                &datastore,
                &folded,
                &context_id,
            )
            .map_err(|e| e.to_string())?
        };
        if tee_keys.is_empty() {
            return Err(unavailable(
                "the namespace has no TEE to open the sealed secret",
            ));
        }
        let namespace = namespace_of(&datastore, &context_id).map_err(|e| e.to_string())?;
        let signing_key =
            signing_key(&datastore, &context_id, &identity).map_err(|e| e.to_string())?;
        let exchange = ask_namespace_tees(
            node_client.clone(),
            namespace,
            context_id,
            identity,
            signing_key,
            request.to_vec(),
            tee_keys,
        );
        let (tx, rx) = std::sync::mpsc::channel();
        drop(runtime.spawn(async move {
            let _ignored = tx.send(exchange.await);
        }));
        match rx.recv_timeout(ANSWER_TIMEOUT * u32::try_from(MAX_PEERS_ASKED).unwrap_or(1)) {
            Ok(Ok(Some(response))) => Ok(response),
            Ok(Ok(None)) => Err(unavailable(
                "no TEE of the namespace could be reached to open the sealed secret",
            )),
            Ok(Err(reason)) => Err(reason),
            Err(_) => Err(unavailable("no TEE of the namespace answered in time")),
        }
    })
}

/// The namespace `context_id` belongs to.
fn namespace_of(datastore: &Store, context_id: &ContextId) -> EyreResult<[u8; 32]> {
    let Some(group_id) = calimero_governance_store::get_group_for_context(datastore, context_id)?
    else {
        bail!("context {context_id} belongs to no namespace");
    };
    Ok(
        calimero_governance_store::NamespaceRepository::new(datastore)
            .resolve(&group_id)?
            .to_bytes(),
    )
}

/// The private key behind `identity` in `context_id`.
fn signing_key(
    datastore: &Store,
    context_id: &ContextId,
    identity: &PublicKey,
) -> EyreResult<PrivateKey> {
    let mut bytes =
        calimero_governance_store::resolve_local_signing_key(datastore, context_id, identity)?
            .ok_or_else(|| eyre!("this node holds no key for {identity} in {context_id}"))?;
    let key = PrivateKey::from(bytes);
    zeroize::Zeroize::zeroize(&mut bytes);
    Ok(key)
}

/// Offers `request` to the namespace's peers in turn until a TEE answers.
///
/// `Ok(None)`: no peer was a TEE that answered. `Err`: a TEE refused it, or
/// sent it and it failed there; asking another would only repeat that.
async fn ask_namespace_tees(
    node_client: NodeClient,
    namespace: [u8; 32],
    context_id: ContextId,
    identity: PublicKey,
    signing_key: PrivateKey,
    request: Vec<u8>,
    tee_keys: Vec<PublicKey>,
) -> Result<Option<Vec<u8>>, String> {
    let network = node_client.network_client();
    let topic = libp2p::gossipsub::TopicHash::from_raw(format!("ns/{}", hex::encode(namespace)));
    let local_peer = network.network_status().await.local_peer_id;
    let pop = signing_key
        .sign(&InitProof::message(
            &context_id,
            &identity,
            &local_peer.to_bytes(),
        ))
        .map_err(|e| e.to_string())?;
    let pop = InitProof {
        signature: pop.to_bytes(),
    };
    let peers = network.subscribed_peers(topic).await;
    for peer in peers.into_iter().take(MAX_PEERS_ASKED) {
        match ask_peer(
            node_client.clone(),
            peer,
            context_id,
            identity,
            pop,
            &request,
        )
        .await
        {
            Ok(answer) => {
                match read_forwarded_answer(context_id.as_ref(), &request, &answer, &tee_keys) {
                    Ok(Some(response)) => return Ok(Some(response)),
                    Ok(None) => {}
                    Err(reason) if answer_is_signed_refusal(&answer) => return Err(reason),
                    Err(reason) => debug!(%peer, %reason, "ignoring a forwarded-request answer"),
                }
            }
            Err(err) => debug!(%peer, %err, "a namespace peer did not take a forwarded request"),
        }
    }
    Ok(None)
}

/// A TEE's refusal ends the search: another TEE would check the same rules.
fn answer_is_signed_refusal(answer: &[u8]) -> bool {
    matches!(
        borsh::from_slice::<ForwardedAnswer>(answer),
        Ok(ForwardedAnswer::Refused { .. })
    )
}

/// One request to one peer, and its raw answer.
async fn ask_peer(
    node_client: NodeClient,
    peer: libp2p::PeerId,
    context_id: ContextId,
    identity: PublicKey,
    pop: InitProof,
    request: &[u8],
) -> EyreResult<Vec<u8>> {
    use calimero_network_primitives::stream::Message;

    let mut stream =
        tokio::time::timeout(OPEN_TIMEOUT, node_client.network_client().open_stream(peer))
            .await
            .map_err(|_| eyre!("opening a stream timed out"))??;
    let init = StreamMessage::Init {
        context_id,
        party_id: identity,
        payload: InitPayload::SealedOutboundRequest {
            context_id,
            request: request.to_vec(),
        },
        next_nonce: {
            use rand::RngExt;
            rand::rng().random()
        },
        pop: Some(pop),
    };
    stream.send(Message::new(borsh::to_vec(&init)?)).await?;
    let reply = tokio::time::timeout(ANSWER_TIMEOUT, stream.next())
        .await
        .map_err(|_| eyre!("the peer did not answer in time"))?
        .ok_or_else(|| eyre!("the peer closed the stream"))??;
    match borsh::from_slice::<StreamMessage<'static>>(&reply.data)? {
        StreamMessage::Message {
            payload: MessagePayload::SealedOutboundAnswer { answer },
            ..
        } => Ok(answer),
        other => bail!("an unexpected reply: {other:?}"),
    }
}

/// What this node answers a forwarded request with, as `responder` in the
/// request's context: [`ForwardedAnswer::NotTee`] unless it is a TEE authority
/// of the namespace, a refusal when any gate fails here, and otherwise the
/// response, sent and signed by `responder`.
///
/// `requester` has already proven its key on the stream and been found a
/// member of the context; whether that membership may write is checked here,
/// since a run that cannot write has nothing to store a response in.
///
/// Blocking: it sends the request. Call it on a blocking thread.
#[allow(clippy::too_many_arguments)]
pub(crate) fn answer_forwarded(
    service: Option<&Arc<OutboundService>>,
    context_client: &calimero_context_client::client::ContextClient,
    node_client: &NodeClient,
    datastore: &Store,
    scope_projections: &Arc<std::sync::RwLock<ScopeProjections>>,
    context_id: ContextId,
    requester: PublicKey,
    responder: PublicKey,
    request: &[u8],
) -> ForwardedAnswer {
    let refused = |reason: String| ForwardedAnswer::Refused { reason };
    let signing_key = match signing_key(datastore, &context_id, &responder) {
        Ok(key) => key,
        Err(_) => return ForwardedAnswer::NotTee,
    };
    let keys = (|| {
        let folded = FoldedProjections(scope_projections);
        let Some(group_id) =
            calimero_governance_store::get_group_for_context(datastore, &context_id)?
        else {
            return Ok(Vec::new());
        };
        calimero_governance_store::tee_outbound_keys(datastore, &folded, &group_id, &signing_key)
    })();
    let keys: Vec<PrivateKey> = match keys {
        Ok(keys) if !keys.is_empty() => keys,
        Ok(_) => return ForwardedAnswer::NotTee,
        Err(err) => return refused(format!("the TEE could not read its keys: {err}")),
    };
    let Some(service) = service else {
        return refused("this TEE node's operator has not turned on outbound requests".to_owned());
    };
    match calimero_governance_store::NamespaceRepository::new(datastore)
        .is_read_only_for_context(&context_id, &requester)
    {
        Ok(false) => {}
        Ok(true) => return refused(format!("{requester} may not write in {context_id}")),
        Err(err) => return refused(format!("the TEE could not read {requester}'s role: {err}")),
    }
    let run = (|| {
        let Some(context) = context_client.get_context(&context_id)? else {
            bail!("this TEE does not hold context {context_id}");
        };
        let Some((app, declared)) = node_client
            .get_application(&context.application_id)?
            .as_ref()
            .and_then(crate::outbound::app_outbound)
        else {
            bail!("the context's app declares no outbound URLs on this TEE");
        };
        let approval = crate::outbound::namespace_approval(datastore, &context_id, &app.signer)?;
        let keys = Arc::new(keys);
        service
            .for_run(
                app,
                declared,
                approval,
                Some(SealedKeys {
                    context_id: *context_id.as_ref(),
                    keys: Arc::new(move || {
                        Ok(keys
                            .iter()
                            .map(|key| PrivateKey::from(*key.as_bytes()))
                            .collect())
                    }),
                    forward: None,
                }),
                format!("{context_id} (for {requester})"),
            )
            .ok_or_else(|| eyre!("the context's app declares no URL this TEE can use"))
    })();
    let run = match run {
        Ok(run) => run,
        Err(err) => return refused(err.to_string()),
    };
    let response = match run.send(request) {
        Ok(response) => response,
        Err(reason) => return refused(reason),
    };
    let message = forwarded_answer_message(context_id.as_ref(), request, &response);
    match signing_key.sign(&message) {
        Ok(signature) => ForwardedAnswer::Sent {
            response,
            tee: *responder,
            signature: signature.to_bytes().to_vec(),
        },
        Err(err) => refused(format!("the TEE could not sign its answer: {err}")),
    }
}
