//! Direct TEE admission: a fleet node and a member that may vouch for it, on a
//! stream between them.
//!
//! A quote admits a node only if it carries a challenge the admitting member
//! chose for that node a moment ago and commits to the credential being
//! admitted. So the exchange has two steps, and either side can start it:
//!
//! - **The node asks.** `fleet-join` dials each of its admitters' addresses in
//!   turn, asks for a challenge ([`InitPayload::TeeAdmissionChallengeRequest`]),
//!   makes a quote over it and sends the admission request. This replaces
//!   hoping that a peer allowed to vouch — an admin, or an admitted TEE — is in
//!   the gossip mesh: when the only such peer is an owner's laptop behind a
//!   relay that mesh forms intermittently, and a miss is silent.
//! - **The member answers a prompt.** The node also broadcasts a prompt on the
//!   namespace topic. A member that hears it offers the node a challenge on a
//!   stream of its own ([`InitPayload::TeeAdmissionChallengeOffer`]); the node
//!   answers with its quote on that stream, and the member verifies it. The
//!   prompt carries nothing to verify and admits nobody.
//!
//! # Why the addresses carry no authority
//!
//! The responder runs [`verify_and_admit`] — the one function every path ends
//! in — so the challenge, the quote, the credential, the namespace's admission
//! policy and the vouching rule are all decided in one place. A peer that is
//! not allowed to vouch says so, and the initiator moves on. An address pointing
//! somewhere hostile costs a dial and a refusal; it cannot produce an admission,
//! because every peer re-checks the voucher when it applies the op.
//!
//! [`verify_and_admit`]: crate::handlers::tee_attestation_admission::verify_and_admit

use calimero_crypto::Nonce;
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::client::TeeAdmissionParams;
use calimero_node_primitives::sync::{
    InitPayload, MessagePayload, StreamMessage, TeeAdmissionOffered,
};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use libp2p::PeerId;
use rand::RngExt;
use tracing::{debug, info, warn};

use super::SyncManager;
use crate::handlers::tee_attestation_admission::{may_vouch, verify_and_admit, TeeAdmissionClaim};
use crate::tee_admission_state::IssueRefusal;

/// How many distinct admitter machines one request will try.
///
/// Same bound, for the same reason, as the invitation path: each unreachable
/// machine costs a full stream-open timeout, and `fleet-join` is itself bounded.
/// A namespace's admins and admitted TEEs are a handful, not dozens.
const MAX_ADMITTER_MACHINES: usize = 8;

/// A quote over `challenge` for the admission `params` describes.
///
/// Blocking hardware work, so it runs off the async workers. Under
/// `mock_tee` the quote is a mock one, which only a build with the
/// `mock-attestation` feature can make; any other result that is a mock quote is
/// refused, so a real deployment never presents one.
async fn attest_admission(
    params: &TeeAdmissionParams,
    challenge: [u8; 32],
) -> eyre::Result<Vec<u8>> {
    let namespace = params.namespace_id;
    let binding = calimero_op_adapter::tee_admission_binding(
        &namespace,
        &namespace,
        &params.public_key,
        &params.account,
    );
    let report_data = calimero_tee_attestation::admission_report_data(&challenge, &binding);
    let mock_tee = params.mock_tee;
    tokio::task::spawn_blocking(move || generate_quote(report_data, mock_tee))
        .await
        .map_err(|err| eyre::eyre!("quote generation task failed: {err}"))?
}

fn generate_quote(report_data: [u8; 64], mock_tee: bool) -> eyre::Result<Vec<u8>> {
    #[cfg(feature = "mock-attestation")]
    let attestation = if mock_tee {
        Ok(calimero_tee_attestation::generate_mock_attestation(
            report_data,
        ))
    } else {
        calimero_tee_attestation::generate_attestation(report_data)
    };
    #[cfg(not(feature = "mock-attestation"))]
    let attestation = {
        if mock_tee {
            eyre::bail!("mock attestation is not compiled into this build");
        }
        calimero_tee_attestation::generate_attestation(report_data)
    };
    let attestation =
        attestation.map_err(|err| eyre::eyre!("could not generate a quote: {err}"))?;

    #[cfg(feature = "mock-attestation")]
    let refuse_mock = attestation.is_mock && !mock_tee;
    #[cfg(not(feature = "mock-attestation"))]
    let refuse_mock = attestation.is_mock;
    if refuse_mock {
        eyre::bail!("TDX attestation required: a mock quote is not accepted for admission");
    }
    Ok(attestation.quote_bytes)
}

impl SyncManager {
    /// Initiator side: ask each address in turn until one peer admits this node.
    ///
    /// Also remembers that this node is waiting to be admitted, so it answers a
    /// challenge a member offers after hearing its prompt.
    ///
    /// Returns the peer that admitted it. An `Err` names every refusal, so a
    /// fleet operator reading the log sees "not a voucher" or "RTMR3 not in
    /// policy allowlist" instead of a bare timeout.
    pub(super) async fn initiate_tee_admission(
        &self,
        params: TeeAdmissionParams,
    ) -> eyre::Result<PeerId> {
        self.node_state.pending_tee_joins.register(params.clone());

        let routes = super::namespace_join::group_admitter_routes(
            &params.admitter_addrs,
            MAX_ADMITTER_MACHINES,
            &self.local_peer_id().await,
        );
        if routes.is_empty() {
            eyre::bail!(
                "no dialable admitter address for namespace {} (need multiaddrs ending in \
                 /p2p/<peer id> naming a peer other than this node); waiting for a member to \
                 answer the prompt",
                hex::encode(params.namespace_id)
            );
        }

        let pop = self
            .build_join_init_pop(params.namespace_id, params.public_key)
            .await;

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

            match self.ask_one_for_tee_admission(peer, &params, pop).await {
                Ok(()) => {
                    info!(
                        %peer,
                        namespace_id = %hex::encode(params.namespace_id),
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
            hex::encode(params.namespace_id),
            refusals.join("; ")
        )
    }

    /// One admission attempt at one peer: get its challenge, make a quote over
    /// it, send the request. `Err` carries the peer's reason, or what went wrong
    /// reaching it.
    async fn ask_one_for_tee_admission(
        &self,
        peer: PeerId,
        params: &TeeAdmissionParams,
        pop: Option<calimero_node_primitives::sync::InitProof>,
    ) -> Result<(), String> {
        let challenge = match self
            .exchange(
                peer,
                InitPayload::TeeAdmissionChallengeRequest {
                    namespace_id: params.namespace_id,
                },
                params.public_key,
                pop,
            )
            .await?
        {
            MessagePayload::TeeAdmissionChallenge { challenge } => challenge,
            MessagePayload::TeeAdmissionResponse { reason, .. } => return Err(reason),
            other => {
                return Err(format!(
                    "unexpected answer to a challenge request: {other:?}"
                ))
            }
        };

        let quote = attest_admission(params, challenge)
            .await
            .map_err(|err| format!("{err:#}"))?;

        match self
            .exchange(
                peer,
                tee_admission_request(params, quote, challenge),
                params.public_key,
                pop,
            )
            .await?
        {
            MessagePayload::TeeAdmissionResponse { admitted, reason } => {
                if admitted {
                    Ok(())
                } else {
                    Err(reason)
                }
            }
            other => Err(format!(
                "unexpected answer to an admission request: {other:?}"
            )),
        }
    }

    /// One `Init` to one peer and its one answer.
    async fn exchange(
        &self,
        peer: PeerId,
        payload: InitPayload,
        public_key: PublicKey,
        pop: Option<calimero_node_primitives::sync::InitProof>,
    ) -> Result<MessagePayload<'static>, String> {
        let mut stream = self
            .open_stream_bounded(peer)
            .await
            .map_err(|e| format!("could not open a stream: {e}"))?;

        let msg = StreamMessage::Init {
            // Namespace-scoped requests carry a sentinel context id; the proof
            // is bound to the namespace instead (see `build_join_init_pop`).
            context_id: ContextId::from([0u8; 32]),
            party_id: public_key,
            payload,
            next_nonce: rand::rng().random(),
            pop,
        };
        crate::sync::stream::send(&mut stream, &msg, None)
            .await
            .map_err(|e| format!("send failed: {e}"))?;

        match crate::sync::stream::recv(&mut stream, None, self.sync_config.timeout).await {
            Ok(Some(StreamMessage::Message { payload, .. })) => Ok(payload),
            // A peer that refused our proof of possession drops the stream.
            Ok(other) => Err(format!(
                "unexpected answer {:?}",
                other.as_ref().map(std::mem::discriminant)
            )),
            Err(e) => Err(format!("no answer: {e}")),
        }
    }

    /// Responder side of a challenge request: issue one to the requester, if
    /// this node may vouch.
    pub(super) async fn handle_tee_challenge_request(
        &self,
        peer_id: PeerId,
        identity: PublicKey,
        namespace_id: [u8; 32],
        stream: &mut Stream,
        nonce: Nonce,
    ) -> eyre::Result<()> {
        let store = self.context_client.datastore_handle().into_inner();
        let payload = if !may_vouch(&store, namespace_id) {
            MessagePayload::TeeAdmissionResponse {
                admitted: false,
                reason: "this node is neither an admin nor an admitted TEE of the namespace, so \
                         it may not vouch; ask another admitter"
                    .to_owned(),
            }
        } else {
            match self
                .node_state
                .tee_challenges
                .issue_requested(namespace_id, peer_id, identity)
            {
                Ok(challenge) => MessagePayload::TeeAdmissionChallenge { challenge },
                Err(IssueRefusal::TooSoon) => MessagePayload::TeeAdmissionResponse {
                    admitted: false,
                    reason: "a challenge was issued to this peer a moment ago; retry shortly"
                        .to_owned(),
                },
            }
        };
        let answer = StreamMessage::Message {
            sequence_id: 0,
            payload,
            next_nonce: nonce,
        };
        crate::sync::stream::send(stream, &answer, None).await?;
        Ok(())
    }

    /// Responder side of an admission request: verify the attestation and admit,
    /// then say what happened.
    pub(super) async fn handle_tee_admission_request(
        &self,
        peer_id: PeerId,
        namespace_id: [u8; 32],
        claim: TeeAdmissionClaim,
        stream: &mut Stream,
        nonce: Nonce,
    ) -> eyre::Result<()> {
        let public_key = claim.public_key;
        // The account the requester's credential names. Read before the claim is
        // consumed below, but trusted only on an `Err`: `verify_and_admit` raises
        // one only after the challenge is spent and the credential is found to
        // certify the key the requester proved. Verifying the credential here
        // instead would cost its root-key chain before any challenge is checked.
        let requester_account = claim.account.statement.account;
        let result = verify_and_admit(
            &self.context_client,
            &self.node_state.tee_challenges,
            peer_id,
            namespace_id,
            claim,
        )
        .await;
        let (admitted, reason) = direct_admission_answer(result, || {
            let store = self.context_client.datastore_handle().into_inner();
            is_tee_member_at_root(&store, namespace_id, &requester_account)
        });

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

    /// Member side of a prompt: offer `peer`, which published it on the
    /// namespace topic, a challenge, and admit it if its quote over that
    /// challenge holds.
    ///
    /// Best effort. A node that is not waiting to be admitted, or that cannot be
    /// reached, answers nothing, and the prompt is sent again.
    pub(crate) async fn offer_tee_challenge(&self, namespace_id: [u8; 32], peer: PeerId) {
        let store = self.context_client.datastore_handle().into_inner();
        if !may_vouch(&store, namespace_id) {
            return;
        }
        let Some(_slot) = self.node_state.tee_challenges.begin_offer() else {
            return;
        };
        let Ok(challenge) = self
            .node_state
            .tee_challenges
            .issue_offered(namespace_id, peer)
        else {
            return;
        };

        let offered = match self
            .exchange(
                peer,
                InitPayload::TeeAdmissionChallengeOffer {
                    namespace_id,
                    challenge,
                },
                // The offer names no identity: nothing in it is the sender's to
                // prove, and the node answers only while it waits to be admitted.
                [0u8; 32].into(),
                None,
            )
            .await
        {
            Ok(MessagePayload::TeeAdmissionOfferReply {
                offered: Some(offered),
            }) => *offered,
            Ok(_) => {
                debug!(%peer, "TEE prompt: the node is not waiting to be admitted here");
                return;
            }
            Err(reason) => {
                debug!(%peer, %reason, "TEE prompt: the node did not answer the offered challenge");
                return;
            }
        };

        let TeeAdmissionOffered {
            quote_bytes,
            public_key,
            account,
            release_version,
        } = offered;
        match verify_and_admit(
            &self.context_client,
            &self.node_state.tee_challenges,
            peer,
            namespace_id,
            TeeAdmissionClaim {
                quote_bytes,
                public_key,
                challenge,
                account,
                release_version,
            },
        )
        .await
        {
            Ok(verdict) => {
                info!(
                    %peer,
                    %public_key,
                    admitted = verdict.admitted(),
                    "TEE answered the challenge offered for its prompt"
                );
            }
            Err(err) => {
                warn!(%peer, %public_key, error = %format!("{err:#}"), "refused a TEE that answered a prompt")
            }
        }
    }

    /// Responder side of an offered challenge: if this node is waiting to be
    /// admitted to the namespace, answer with a quote over the challenge for its
    /// own credential.
    pub(super) async fn handle_tee_challenge_offer(
        &self,
        peer_id: PeerId,
        namespace_id: [u8; 32],
        challenge: [u8; 32],
        stream: &mut Stream,
        nonce: Nonce,
    ) -> eyre::Result<()> {
        let offered = match self
            .node_state
            .pending_tee_joins
            .take_for_attestation(namespace_id, peer_id)
        {
            None => None,
            Some(params) => match attest_admission(&params, challenge).await {
                Ok(quote_bytes) => Some(Box::new(TeeAdmissionOffered {
                    quote_bytes,
                    public_key: params.public_key,
                    account: params.account.clone(),
                    release_version: params.release_version.clone(),
                })),
                Err(err) => {
                    warn!(%peer_id, error = %format!("{err:#}"), "could not attest for an offered challenge");
                    None
                }
            },
        };
        let answer = StreamMessage::Message {
            sequence_id: 0,
            payload: MessagePayload::TeeAdmissionOfferReply { offered },
            next_nonce: nonce,
        };
        crate::sync::stream::send(stream, &answer, None).await?;
        Ok(())
    }
}

/// What to tell a TEE that asked directly to be admitted.
///
/// `fleet-join` asks its admitters directly while a member that heard its
/// prompt may be offering it a challenge, so the two race. When the prompt's
/// answer wins, this node has already admitted the requester, and the direct
/// request is then judged for a member: its fresh quote may trigger an evidence
/// refresh, which can be refused for a fault in publishing it. The requester
/// would read that as "not admitted" and wait out its whole admission window
/// for a membership it already has.
///
/// So a refusal is checked against what it is about: if the requester is in
/// fact a TEE member of the namespace, it is told so. Only a refusal is
/// re-checked, and only by `already_in`; a refusal is an `Err` only after
/// [`verify_and_admit`] spent the challenge and found the requester's credential
/// certifies the key it proved possession of, so the account it names is the
/// requester's — an
/// ordinary verdict (not a voucher, invalid attestation, spent challenge,
/// foreign credential) is reported as it is.
fn direct_admission_answer(
    result: eyre::Result<crate::handlers::tee_attestation_admission::TeeAdmissionVerdict>,
    already_in: impl FnOnce() -> bool,
) -> (bool, String) {
    match result {
        Ok(verdict) => (verdict.admitted(), verdict.reason()),
        Err(_) if already_in() => (true, String::new()),
        // A policy refusal or a fault. Its text is what the requester needs —
        // "RTMR3 not in policy allowlist" is an instruction to its owner.
        Err(err) => (false, format!("{err:#}")),
    }
}

/// Whether `account` holds a TEE role at the root of `namespace_id`.
///
/// A store read that fails answers `false`: the refusal it would override is
/// then reported unchanged, which is what happened before this check existed.
fn is_tee_member_at_root(
    store: &calimero_store::Store,
    namespace_id: [u8; 32],
    account: &calimero_account::AccountId,
) -> bool {
    calimero_governance_store::MembershipRepository::new(store)
        .role_of(
            &calimero_context_config::types::ContextGroupId::from(namespace_id),
            account,
        )
        .is_ok_and(|role| role.is_some_and(|role| role.is_tee()))
}

/// The request form to send: the one naming this node's release when it knows
/// it.
fn tee_admission_request(
    params: &TeeAdmissionParams,
    quote_bytes: Vec<u8>,
    challenge: [u8; 32],
) -> InitPayload {
    let account = params.account.clone();
    match params.release_version.clone() {
        Some(release_version) => InitPayload::TeeReleaseAdmissionRequest {
            namespace_id: params.namespace_id,
            quote_bytes,
            public_key: params.public_key,
            challenge,
            account,
            release_version,
        },
        None => InitPayload::TeeAdmissionRequest {
            namespace_id: params.namespace_id,
            quote_bytes,
            public_key: params.public_key,
            challenge,
            account,
        },
    }
}

#[cfg(test)]
mod tests {
    use calimero_context_client::group::TeeAdmissionOutcome;

    use super::*;
    use crate::handlers::tee_attestation_admission::TeeAdmissionVerdict;

    fn params(release_version: Option<&str>) -> TeeAdmissionParams {
        TeeAdmissionParams {
            namespace_id: [1; 32],
            admitter_addrs: Vec::new(),
            public_key: [2; 32].into(),
            account: calimero_context::test_support::credential(&[2; 32].into()),
            release_version: release_version.map(str::to_owned),
            mock_tee: false,
        }
    }

    #[test]
    fn the_request_names_the_release_when_the_node_knows_it() {
        assert!(matches!(
            tee_admission_request(&params(None), vec![], [3; 32]),
            InitPayload::TeeAdmissionRequest { challenge, .. } if challenge == [3; 32]
        ));
        assert!(matches!(
            tee_admission_request(&params(Some("2.3.72")), vec![], [3; 32]),
            InitPayload::TeeReleaseAdmissionRequest { release_version, .. }
                if release_version == "2.3.72"
        ));
    }

    #[cfg(not(feature = "mock-attestation"))]
    #[test]
    fn a_build_without_mock_attestation_refuses_to_make_a_mock_quote() {
        let err = generate_quote([0; 64], true).expect_err("no mock in this build");
        assert!(err.to_string().contains("not compiled"), "{err}");
    }

    /// The fleet-join race: a member answering the node's prompt admitted it
    /// first, so its direct request is refused (here, by the evidence refresh
    /// it triggered). The node IS in, and is told so rather than left to wait
    /// out its admission window.
    #[test]
    fn a_refusal_for_a_node_already_admitted_answers_admitted() {
        let (admitted, reason) = direct_admission_answer(
            Err(eyre::eyre!("could not publish the refreshed evidence")),
            || true,
        );
        assert!(admitted);
        assert!(reason.is_empty(), "{reason}");
    }

    /// The same refusal for a node that is NOT a member stays a refusal, with
    /// its reason.
    #[test]
    fn a_refusal_for_a_node_not_admitted_stays_a_refusal() {
        let (admitted, reason) = direct_admission_answer(
            Err(eyre::eyre!("TEE attestation quote already used")),
            || false,
        );
        assert!(!admitted);
        assert!(reason.contains("already used"), "{reason}");
    }

    /// An ordinary verdict is reported as it is, without consulting membership.
    #[test]
    fn an_ordinary_verdict_is_not_overridden() {
        let (admitted, reason) = direct_admission_answer(
            Ok(TeeAdmissionVerdict::Decided(
                TeeAdmissionOutcome::NotAVoucher,
            )),
            || panic!("a verdict is not re-checked"),
        );
        assert!(!admitted);
        assert!(reason.contains("may not"), "{reason}");

        let (admitted, _) =
            direct_admission_answer(Ok(TeeAdmissionVerdict::AttestationInvalid), || {
                panic!("a verdict is not re-checked")
            });
        assert!(!admitted);
    }
}

#[cfg(test)]
mod initiator_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use calimero_node_primitives::client::TeeAdmissionParams;
    use calimero_primitives::identity::PublicKey;
    use libp2p::PeerId;

    use crate::sync::manager::namespace_sync::group_key_recovery_anchor_tests::manager;
    use crate::sync::network::mock::MockSyncNetwork;

    fn params() -> TeeAdmissionParams {
        TeeAdmissionParams {
            namespace_id: [0x7E; 32],
            admitter_addrs: Vec::new(),
            public_key: PublicKey::from([0x11; 32]),
            account: calimero_governance_store::test_fixtures::real_join_account(&PublicKey::from(
                [0x11; 32],
            )),
            release_version: None,
            mock_tee: false,
        }
    }

    /// An admitter whose stream open is never answered costs the open budget,
    /// not forever. libp2p leaves the open pending when the dial it waits on
    /// fails without a connection — a dial to this node's own peer id does —
    /// and fleet-join, and the sync loop running the request, waited with it.
    #[tokio::test(start_paused = true)]
    async fn a_stream_open_that_never_answers_gives_up_at_the_open_budget() {
        let mock = Arc::new(MockSyncNetwork::default());
        let _ = mock.push_open_stream_hang(Duration::from_secs(24 * 60 * 60), "never");
        let (sync_manager, _store, _tmp) = manager(Arc::clone(&mock)).await;
        let budget = sync_manager.sync_config.open_stream_timeout;

        let started = tokio::time::Instant::now();
        let refused = sync_manager
            .ask_one_for_tee_admission(PeerId::random(), &params(), None)
            .await
            .expect_err("a peer that never answers does not admit us");

        assert!(
            started.elapsed() <= budget + Duration::from_secs(1),
            "gave up after {:?}, not at the {budget:?} open budget",
            started.elapsed()
        );
        assert!(refused.contains("timed out"), "{refused}");
    }
}

#[cfg(test)]
mod membership_tests {
    use std::sync::Arc;

    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::MembershipRepository;
    use calimero_primitives::context::GroupMemberRole;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;

    use super::is_tee_member_at_root;

    /// Only a TEE row at the namespace root makes a refused requester "in": a
    /// plain member, or nobody at all, is not overridden.
    #[test]
    fn only_a_tee_row_at_the_root_counts_as_admitted() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ns = [0x4E; 32];
        let gid = ContextGroupId::from(ns);
        let tee = calimero_account::AccountId::from([0x01; 32]);
        let member = calimero_account::AccountId::from([0x02; 32]);
        let stranger = calimero_account::AccountId::from([0x03; 32]);
        let membership = MembershipRepository::new(&store);
        membership
            .add_member(&gid, &tee, GroupMemberRole::RelayTee)
            .expect("seat the TEE");
        membership
            .add_member(&gid, &member, GroupMemberRole::Member)
            .expect("seat the member");

        assert!(is_tee_member_at_root(&store, ns, &tee));
        assert!(!is_tee_member_at_root(&store, ns, &member));
        assert!(!is_tee_member_at_root(&store, ns, &stranger));
    }
}
