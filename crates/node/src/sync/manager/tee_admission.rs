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
use crate::handlers::tee_attestation_admission::TeeAdmissionClaim;

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
            release_version,
        } = params;

        let routes = super::namespace_join::group_admitter_routes(
            &admitter_addrs,
            MAX_ADMITTER_MACHINES,
            &self.local_peer_id().await,
        );
        if routes.is_empty() {
            eyre::bail!(
                "no dialable admitter address for namespace {} (need multiaddrs ending in \
                 /p2p/<peer id> naming a peer other than this node)",
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
                    tee_admission_request(
                        namespace_id,
                        public_key,
                        &quote_bytes,
                        nonce,
                        &account,
                        release_version.as_deref(),
                    ),
                    public_key,
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
    async fn ask_one_for_tee_admission(
        &self,
        peer: PeerId,
        payload: InitPayload,
        public_key: PublicKey,
        pop: Option<calimero_node_primitives::sync::InitProof>,
    ) -> Result<(), String> {
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
    pub(super) async fn handle_tee_admission_request(
        &self,
        peer_id: PeerId,
        namespace_id: [u8; 32],
        claim: TeeAdmissionClaim,
        stream: &mut Stream,
        nonce: Nonce,
    ) -> eyre::Result<()> {
        let public_key = claim.public_key;
        // Who the requester is, if its credential is for the key it proved
        // possession of. Read before the claim is consumed below.
        let requester_account =
            calimero_op_adapter::join_credential_certifies(&public_key, &claim.account)
                .then_some(claim.account.statement.account);
        let result = crate::handlers::tee_attestation_admission::verify_and_admit(
            &self.context_client,
            peer_id,
            namespace_id,
            claim,
        )
        .await;
        let (admitted, reason) = direct_admission_answer(result, || {
            requester_account.is_some_and(|account| {
                let store = self.context_client.datastore_handle().into_inner();
                is_tee_member_at_root(&store, namespace_id, &account)
            })
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
}

/// What to tell a TEE that asked directly to be admitted.
///
/// `fleet-join` sends the direct request with the same quote and nonce as its
/// broadcast, so the two race. When the broadcast wins, this node has already
/// admitted the requester with that quote, and the direct request is then
/// refused for it — "TEE attestation quote already used" — or for a fault in
/// publishing evidence the admission already carried. The requester would read
/// that as "not admitted" and wait out its whole admission window for a
/// membership it already has.
///
/// So a refusal is checked against what it is about: if the requester is in
/// fact a TEE member of the namespace, it is told so. Only a refusal is
/// re-checked, and only by `already_in`, which the caller confines to a
/// requester whose credential certifies the key it proved possession of — an
/// ordinary verdict (not a voucher, invalid attestation, foreign credential)
/// is reported as it is.
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
/// it. A responder that predates that form drops the stream, and the initiator
/// moves on to the next admitter and then to the broadcast, which also carries
/// the old form.
fn tee_admission_request(
    namespace_id: [u8; 32],
    public_key: PublicKey,
    quote_bytes: &[u8],
    nonce: [u8; 32],
    account: &calimero_governance_types::JoinAccountCredential,
    release_version: Option<&str>,
) -> InitPayload {
    let quote_bytes = quote_bytes.to_vec();
    let account = Box::new(account.clone());
    match release_version {
        Some(release_version) => InitPayload::TeeReleaseAdmissionRequest {
            namespace_id,
            quote_bytes,
            public_key,
            nonce,
            account,
            release_version: release_version.to_owned(),
        },
        None => InitPayload::TeeAdmissionRequest {
            namespace_id,
            quote_bytes,
            public_key,
            nonce,
            account,
        },
    }
}

#[cfg(test)]
mod tests {
    use calimero_context_client::group::TeeAdmissionOutcome;

    use super::direct_admission_answer;
    use crate::handlers::tee_attestation_admission::TeeAdmissionVerdict;

    /// The fleet-join race: the broadcast admitted the node first, so its direct
    /// request with the same quote is refused as a replay. The node IS in, and
    /// is told so rather than left to wait out its admission window.
    #[test]
    fn a_replayed_quote_from_a_node_already_admitted_answers_admitted() {
        let (admitted, reason) = direct_admission_answer(
            Err(eyre::eyre!("TEE attestation quote already used")),
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

    use calimero_node_primitives::sync::InitPayload;
    use calimero_primitives::identity::PublicKey;
    use libp2p::PeerId;

    use crate::sync::manager::namespace_sync::group_key_recovery_anchor_tests::manager;
    use crate::sync::network::mock::MockSyncNetwork;

    fn request() -> InitPayload {
        InitPayload::TeeAdmissionRequest {
            namespace_id: [0x7E; 32],
            quote_bytes: Vec::new(),
            public_key: PublicKey::from([0x11; 32]),
            nonce: [0x22; 32],
            account: calimero_governance_store::test_fixtures::real_join_account(&PublicKey::from(
                [0x11; 32],
            )),
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
            .ask_one_for_tee_admission(
                PeerId::random(),
                request(),
                PublicKey::from([0x11; 32]),
                None,
            )
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
