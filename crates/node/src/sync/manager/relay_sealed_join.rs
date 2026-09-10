//! Relay of a join its own author cannot seal (#3904).
//!
//! Both halves of one exchange: the joiner asks, a namespace keyholder seals and
//! publishes.
//!
//! # Why the exchange exists
//!
//! `MemberJoined` / `MemberJoinedAt` name an account, the group it joined and
//! when. Published in the clear on the namespace topic, that is the membership
//! graph the whole sealing series (#3847, #3850, #3856-#3859) exists to keep off
//! the wire.
//!
//! A joiner that already holds the covering key seals its own join and needs
//! none of this. A joiner that does not cannot be told to seal anyway: the key
//! it lacks arrives in a `KeyDelivery` an admin publishes *on seeing this very
//! op*, so there is no order in which it could seal for itself. Before this
//! module the answer was to publish in the clear and accept the disclosure.
//!
//! # Why routing it through the admitter costs nothing
//!
//! Since #3804 a join with no admitter endorsement fails outright — every peer
//! refuses an unendorsed `MemberJoined` at apply. So by the time a joiner has
//! anything worth publishing it has *already* completed a stream exchange with
//! an admitter, and asking that same peer to wrap the op introduces no
//! reachability requirement the join did not already have.
//!
//! It is not guaranteed to hold the key, though, and that is worth being exact
//! about because it is how a joiner ends up unkeyed in the first place: an
//! admitter still awaiting its own `KeyDelivery` endorses the join and answers
//! with an empty key envelope. So the initiator falls through to the rest of the
//! namespace topic after a refusal. Widening is sound because a relayer supplies
//! a seal and an envelope signature, never authority — the endorsement is inside
//! the op and self-authenticating.
//!
//! # Why the relayer cannot abuse it
//!
//! It wraps; it never re-authors. `NamespaceOp::RootRelaySealed` carries the
//! joiner's whole `SignedNamespaceOp`, and governance-store's
//! `open_relayed_join` verifies that inner signature after decrypting — the
//! outer envelope's signature proves only who relayed. `join_op_proves_ownership` then requires the signer to be the
//! account's own device key, so a relayer that swapped the member for one of its
//! own would produce an op every peer rejects.
//!
//! # Why there is no dedup on the responder
//!
//! A replayed endorsed join reaches this handler as a valid request, and it is
//! accepted: the responder seals and publishes it, and the apply path is what
//! refuses it — `contains_op` on the INNER op for one that also arrived in the
//! clear, and the per-signer nonce window for one that only ever arrived
//! relayed. Those two are what stop a re-sealed old join resurrecting a removed
//! member, and they are tested where they live.
//!
//! Refusing here on "already applied locally" looks like a free improvement and
//! is not. A joiner whose relay succeeded but whose *response* was lost retries
//! against another peer, which by then may already hold the op from gossip — so
//! that check would fail a join that in fact succeeded. Answering `accepted` for
//! an already-known op would be correct but is the same outcome as publishing a
//! duplicate the apply path drops, at more complexity. The cost of not checking
//! is one signature and one publish per replayed request, which is the same cost
//! any namespace peer can impose by publishing to the topic directly.

use calimero_crypto::Nonce;
use calimero_governance_types::{NamespaceOp, RootOp, SignedNamespaceOp};
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::client::RelaySealedJoinParams;
use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};
use calimero_primitives::context::ContextId;
use libp2p::PeerId;
use rand::RngExt;
use tracing::{debug, info, warn};

use super::SyncManager;

/// What a decoded relay request must be before this node will seal it.
///
/// Checked on the responder rather than left to the apply path, and not because
/// the apply path is unreliable — it re-checks all of this. The point is that a
/// refusal here reaches the joiner as a reason on an open stream, where it can
/// act on it, instead of as a published op that silently never applies anywhere.
///
/// Deliberately NOT a membership or authority check on the requester. The
/// authority in this exchange is the endorsement inside the op, which is
/// self-authenticating; the peer carrying it needs none of its own. A relay
/// request from a stranger holding a validly endorsed join is exactly as
/// admissible as one from the joiner, because it publishes the same op.
fn decode_relay_request(
    namespace_id: [u8; 32],
    signed_op_bytes: &[u8],
) -> Result<SignedNamespaceOp, String> {
    let op: SignedNamespaceOp =
        borsh::from_slice(signed_op_bytes).map_err(|e| format!("undecodable signed op: {e}"))?;

    if op.namespace_id.to_bytes() != namespace_id {
        return Err(format!(
            "op is for namespace {}, not the {} this request names",
            hex::encode(op.namespace_id.as_bytes()),
            hex::encode(namespace_id)
        ));
    }

    // A relay carries a join and nothing else. Sealing on someone's behalf is
    // authority to admit them, not authority to have arbitrary governance
    // published under this node's signature in a form peers cannot read.
    if !matches!(
        op.op,
        NamespaceOp::Root(RootOp::MemberJoined { .. } | RootOp::MemberJoinedAt { .. })
    ) {
        return Err(format!(
            "op is {}, but a relay carries an invitation join only",
            op.op.op_kind_label()
        ));
    }

    op.validate().map_err(|e| format!("invalid op: {e}"))?;
    op.verify_signature()
        .map_err(|e| format!("signature did not verify: {e}"))?;

    // No endorsement, nothing to relay. Every peer refuses an unendorsed join at
    // apply, so sealing one would spend this node's signature on an op that
    // cannot take effect — and leave the joiner waiting on a membership that
    // never materialises rather than reading the reason here.
    if op.admitter_endorsement.is_none() {
        return Err(
            "op carries no admitter endorsement, which every peer requires to admit a join"
                .to_owned(),
        );
    }

    Ok(op)
}

impl SyncManager {
    /// Initiator side: hand `params.signed_op_bytes` to a keyholder to seal and
    /// publish.
    ///
    /// Tries the endorsing admitter first — it is known reachable, since the
    /// endorsement arrived over a stream to it — then any other peer on the
    /// namespace topic. Widening past the admitter is sound because the
    /// endorsement travels inside the op: a relayer contributes an envelope
    /// signature and a seal, never authority.
    ///
    /// An `Err` fails the caller's join, by design. The alternative is the
    /// cleartext publish this path replaces, and a fallback that quietly
    /// re-opens the disclosure is worse than a join the operator can retry.
    pub(super) async fn initiate_relay_sealed_join(
        &self,
        params: RelaySealedJoinParams,
    ) -> eyre::Result<()> {
        let RelaySealedJoinParams {
            namespace_id,
            admitter_peer,
            joiner_public_key,
            signed_op_bytes,
        } = params;

        let join_pop = self
            .build_join_init_pop(namespace_id, joiner_public_key)
            .await;

        let topic =
            libp2p::gossipsub::TopicHash::from_raw(format!("ns/{}", hex::encode(namespace_id)));

        // The admitter first, then everyone else on the topic, with the admitter
        // not tried twice.
        let mut peers: Vec<PeerId> = admitter_peer.into_iter().collect();
        for peer in self.sync_network.subscribed_peers(topic).await {
            if !peers.contains(&peer) {
                peers.push(peer);
            }
        }

        if peers.is_empty() {
            eyre::bail!(
                "no peer to relay this join to for namespace {}: it cannot be sealed locally and \
                 publishing it in the clear would disclose the membership",
                hex::encode(namespace_id)
            );
        }

        let mut refusals: Vec<String> = Vec::new();
        let mut transport_errors = 0usize;

        for peer in &peers {
            let mut stream = match self.sync_network.open_stream(*peer).await {
                Ok(s) => s,
                Err(e) => {
                    debug!(
                        peer = %peer,
                        error = %e,
                        "relay-sealed join: failed to open stream, trying next peer"
                    );
                    transport_errors += 1;
                    continue;
                }
            };

            let msg = StreamMessage::Init {
                // Join `Init`s carry a sentinel context id; the proof below is
                // bound to the namespace instead (see `build_join_init_pop`).
                context_id: ContextId::from([0u8; 32]),
                party_id: joiner_public_key,
                payload: InitPayload::RelaySealedJoinRequest {
                    namespace_id,
                    signed_op_bytes: signed_op_bytes.clone(),
                },
                next_nonce: rand::rng().random(),
                pop: join_pop,
            };

            if let Err(e) = crate::sync::stream::send(&mut stream, &msg, None).await {
                debug!(
                    peer = %peer,
                    error = %e,
                    "relay-sealed join: send failed, trying next peer"
                );
                transport_errors += 1;
                continue;
            }

            match crate::sync::stream::recv(&mut stream, None, self.sync_config.timeout).await {
                Ok(Some(StreamMessage::Message {
                    payload: MessagePayload::RelaySealedJoinResponse { accepted, reason },
                    ..
                })) => {
                    if accepted {
                        info!(
                            peer = %peer,
                            namespace_id = %hex::encode(namespace_id),
                            "join sealed and published by a relaying keyholder"
                        );
                        return Ok(());
                    }
                    // A peer that holds no key for this namespace refuses, and
                    // another may well hold one — keep the reason and move on.
                    debug!(
                        peer = %peer,
                        reason = %reason,
                        "relay-sealed join: peer refused, trying next peer"
                    );
                    refusals.push(format!("{peer}: {reason}"));
                    continue;
                }
                Ok(other) => {
                    // Includes the older-build case: a responder that cannot
                    // decode this payload drops the stream. Counted as a
                    // transport error, which fails the join — never a silent
                    // downgrade to a cleartext publish.
                    debug!(
                        peer = %peer,
                        "relay-sealed join: unexpected response {:?}, trying next peer",
                        other.as_ref().map(std::mem::discriminant)
                    );
                    transport_errors += 1;
                    continue;
                }
                Err(e) => {
                    debug!(
                        peer = %peer,
                        error = %e,
                        "relay-sealed join: recv failed, trying next peer"
                    );
                    transport_errors += 1;
                    continue;
                }
            }
        }

        eyre::bail!(
            "no peer would relay this join for namespace {}: tried {} peer(s), {} transport \
             error(s), refusals: [{}]",
            hex::encode(namespace_id),
            peers.len(),
            transport_errors,
            refusals.join("; ")
        )
    }

    /// Responder side: seal the joiner's op under the namespace key and publish
    /// it.
    ///
    /// The sealing and the publish both live in the context actor — it holds the
    /// keyring and this node's namespace signing key — so this method's whole
    /// job is to refuse what should not be sealed and to answer on the stream.
    pub(super) async fn handle_relay_sealed_join_request(
        &self,
        namespace_id: [u8; 32],
        signed_op_bytes: &[u8],
        stream: &mut Stream,
        nonce: Nonce,
    ) -> eyre::Result<()> {
        let answer = |accepted: bool, reason: String| StreamMessage::Message {
            sequence_id: 0,
            payload: MessagePayload::RelaySealedJoinResponse { accepted, reason },
            next_nonce: nonce,
        };

        let op = match decode_relay_request(namespace_id, signed_op_bytes) {
            Ok(op) => op,
            Err(reason) => {
                warn!(
                    namespace_id = %hex::encode(namespace_id),
                    reason = %reason,
                    "refusing to relay a join"
                );
                crate::sync::stream::send(stream, &answer(false, reason), None).await?;
                return Ok(());
            }
        };

        // `relay_signed_join` refuses rather than downgrades when this node
        // holds no namespace key, so a keyless peer answers "ask someone else"
        // instead of publishing the join in the clear on the joiner's behalf.
        match self.context_client.relay_signed_join(op).await {
            Ok(()) => {
                info!(
                    namespace_id = %hex::encode(namespace_id),
                    "sealed and published a relayed join"
                );
                crate::sync::stream::send(stream, &answer(true, String::new()), None).await?;
            }
            Err(e) => {
                let reason = format!("{e:#}");
                warn!(
                    namespace_id = %hex::encode(namespace_id),
                    error = %reason,
                    "could not seal and publish a relayed join"
                );
                crate::sync::stream::send(stream, &answer(false, reason), None).await?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use calimero_context_client::local_governance::JoinAccountCredential;
    use calimero_context_config::types::{
        ContextGroupId, GroupInvitationFromAdmin, SignedGroupOpenInvitation, SignerId,
    };
    use calimero_governance_types::AdmitterEndorsement;
    use calimero_primitives::identity::{PrivateKey, PublicKey};

    use super::*;

    const NS: [u8; 32] = [7u8; 32];

    /// The joiner's key. Signs the op, so it must be a real keypair — three of
    /// the four tests below turn on `verify_signature` either passing or not.
    fn joiner_sk() -> PrivateKey {
        PrivateKey::from([0x4Du8; 32])
    }

    fn credential() -> Box<JoinAccountCredential> {
        let root = PublicKey::from([0x7Au8; 32]);
        let genesis = calimero_account::AccountGenesis::new(root);
        Box::new(JoinAccountCredential {
            statement: calimero_account::DeviceCert {
                account: genesis.account_id(),
                device: calimero_account::DeviceId::from([0x3Eu8; 32]),
                sign_pk: joiner_sk().public_key(),
                kem_pk: calimero_account::KemPublicKey::from([0x2Bu8; 32]),
                key_epoch: 0,
                device_epoch: 0,
                signature: [0x11u8; 64],
            },
            genesis,
            chain: vec![],
        })
    }

    fn invitation() -> SignedGroupOpenInvitation {
        SignedGroupOpenInvitation {
            inviter_account: None,
            invitation: GroupInvitationFromAdmin {
                inviter_identity: SignerId::from([1u8; 32]),
                group_id: ContextGroupId::from(NS),
                expiration_timestamp: 0,
                invitation_nonce: [2u8; 32],
                invited_role: 1,
                admitters: Vec::new(),
            },
            inviter_signature: String::new(),
            application_id: None,
            bytecode_id: None,
            admitter_addrs: Vec::new(),
        }
    }

    fn endorsement() -> Box<AdmitterEndorsement> {
        Box::new(
            AdmitterEndorsement::sign(
                &PrivateKey::from([5u8; 32]),
                &NS,
                &credential().statement.account,
                &invitation().invitation.invitation_nonce,
            )
            .expect("sign the endorsement"),
        )
    }

    /// A relay request the responder should accept: a real joiner signature over
    /// a `MemberJoinedAt` for this namespace, endorsement attached.
    ///
    /// Deliberately built through `SignedNamespaceOp::sign` rather than by
    /// filling the struct: the signature is what three of these tests are about,
    /// and a hand-filled one would make them pass for the wrong reason.
    fn endorsed_join(op: NamespaceOp) -> SignedNamespaceOp {
        let mut signed = SignedNamespaceOp::sign(&joiner_sk(), NS.into(), Vec::new(), 1, op)
            .expect("sign the join");
        signed.admitter_endorsement = Some(endorsement());
        signed
    }

    fn join_op() -> NamespaceOp {
        NamespaceOp::Root(RootOp::MemberJoinedAt {
            member: credential().statement.account,
            signed_invitation: invitation(),
            joined_at: 0,
            account: credential(),
        })
    }

    fn bytes(op: &SignedNamespaceOp) -> Vec<u8> {
        borsh::to_vec(op).expect("borsh the signed op")
    }

    /// The shape gate refuses an op for another namespace, and that is the check
    /// that stops a relay being a way to smuggle an op into a DAG it was not
    /// signed for: the outer envelope this node would mint names ITS namespace,
    /// so nothing further out ever sees the mismatch.
    #[test]
    fn refuses_an_op_signed_for_another_namespace() {
        let op = endorsed_join(join_op());
        let other = {
            let mut id = NS;
            id[0] ^= 0xff;
            id
        };

        let err = decode_relay_request(other, &bytes(&op))
            .expect_err("an op for another namespace must be refused");
        assert!(err.contains("not the"), "unexpected reason: {err}");

        // The same bytes are accepted under their own namespace, so the refusal
        // above is about the namespace and not about the fixture.
        let _accepted =
            decode_relay_request(NS, &bytes(&op)).expect("the op's own namespace must be accepted");
    }

    /// An unendorsed join is refused here rather than sealed and published,
    /// because every peer refuses it at apply — sealing it would spend this
    /// node's signature on an op that cannot take effect and leave the joiner
    /// waiting instead of reading the reason.
    #[test]
    fn refuses_a_join_with_no_endorsement() {
        let mut op = endorsed_join(join_op());
        op.admitter_endorsement = None;

        let err =
            decode_relay_request(NS, &bytes(&op)).expect_err("an unendorsed join must be refused");
        assert!(err.contains("endorsement"), "unexpected reason: {err}");
    }

    /// The inner signature is checked before anything is sealed. Without this an
    /// admitter could wrap a join whose signature does not verify: peers would
    /// reject it at apply, but the joiner would have been told its join
    /// succeeded, and the refusal would surface nowhere.
    #[test]
    fn refuses_a_join_whose_signature_does_not_verify() {
        let mut op = endorsed_join(join_op());
        op.signature[0] ^= 0xff;

        let err =
            decode_relay_request(NS, &bytes(&op)).expect_err("a bad signature must be refused");
        assert!(err.contains("signature"), "unexpected reason: {err}");
    }

    /// The signature is over the op, so re-pointing the join at a different
    /// member invalidates it. This is the substitution the relay path has to be
    /// unable to perform: an admitter can wrap, and wrapping does not let it
    /// change who joined.
    #[test]
    fn an_admitter_cannot_substitute_the_member() {
        let mut op = endorsed_join(join_op());
        let NamespaceOp::Root(RootOp::MemberJoinedAt { member, .. }) = &mut op.op else {
            panic!("the fixture is a MemberJoinedAt");
        };
        *member = calimero_account::AccountId::from([0x99u8; 32]);

        let err = decode_relay_request(NS, &bytes(&op))
            .expect_err("a substituted member must invalidate the joiner's signature");
        assert!(err.contains("signature"), "unexpected reason: {err}");
    }

    /// A relay carries a join only. The seal makes the payload unreadable to
    /// non-keyholders, so accepting an arbitrary root op here would turn
    /// "authority to admit this person" into "authority to publish governance
    /// nobody outside the namespace can inspect".
    #[test]
    fn refuses_a_root_op_that_is_not_a_join() {
        let op = endorsed_join(NamespaceOp::Root(RootOp::GroupCreated {
            group_id: ContextGroupId::from([8u8; 32]),
            parent_id: ContextGroupId::from(NS),
            restricted: true,
            admin: credential().statement.account,
        }));

        let err =
            decode_relay_request(NS, &bytes(&op)).expect_err("a non-join root op must be refused");
        assert!(err.contains("join only"), "unexpected reason: {err}");
    }
}
