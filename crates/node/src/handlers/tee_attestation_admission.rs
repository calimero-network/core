//! TEE attestation-based admission handler.
//!
//! A fleet TEE node is admitted only on a quote that carries a challenge this
//! node issued to it and commits to the credential being admitted. The quote
//! reaches here in a direct admission request, or in the reply to a challenge
//! this node offered after hearing the node's prompt on the namespace topic.
//! [`verify_and_admit`] spends the challenge, verifies the quote against the
//! group's `TeeAdmissionPolicy` and, if valid, admits the node via a
//! `MemberJoinedViaTeeAttestation` governance op.
//!
//! The heavy lifting (policy lookup, governance op signing, DAG interaction) is
//! delegated to `calimero_governance_store` via the `ContextClient`.
use calimero_context_client::group::TeeAdmissionOutcome;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::identity::PublicKey;
use calimero_tee_attestation::verify_attestation;
#[cfg(feature = "mock-attestation")]
use calimero_tee_attestation::{is_mock_quote, verify_mock_attestation};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::tee_admission_state::TeeChallenges;

/// Whether this node may vouch for a TEE in `namespace_id`: it is an admin of
/// the namespace or an admitted TEE in it. Peers refuse an admission signed by
/// anyone else, so a node that may not vouch neither offers a challenge nor
/// issues one.
///
/// A cheap read of local state. `admit_tee_node` decides authoritatively when an
/// admission is published.
pub(crate) fn may_vouch(store: &calimero_store::Store, namespace_id: [u8; 32]) -> bool {
    let namespace = ContextGroupId::from(namespace_id);
    let Ok(Some((identity, _))) =
        calimero_governance_store::NamespaceRepository::new(store).resolve_identity(&namespace)
    else {
        return false;
    };
    let Ok(Some(account)) =
        calimero_governance_store::member_account_in_namespace(store, &namespace, &identity)
    else {
        return false;
    };
    calimero_governance_store::MembershipPolicy::new(store, namespace)
        .is_tee_attestation_verifier(&account)
        .unwrap_or(false)
}

/// What a TEE sends to be admitted, however it arrives: a direct request
/// (`TeeAdmissionRequest` / `TeeReleaseAdmissionRequest`) or the reply to a
/// challenge offered to it.
#[derive(Debug)]
pub(crate) struct TeeAdmissionClaim {
    /// TDX quote whose `report_data` is `challenge` followed by the admission
    /// binding of `account` and `public_key`.
    pub quote_bytes: Vec<u8>,
    /// The TEE's namespace identity.
    pub public_key: PublicKey,
    /// The challenge this node issued to the TEE.
    pub challenge: [u8; 32],
    /// The TEE's account credential; must certify `public_key`.
    pub account: Box<calimero_context_client::local_governance::JoinAccountCredential>,
    /// The mero-tee node release it says it runs, when its form carries one.
    pub release_version: Option<String>,
}

/// What became of one TEE admission request, however it arrived.
///
/// The broadcast receiver and the direct-request responder both go through
/// [`verify_and_admit`], so the rule for who gets in is written once. They
/// differ only in what they do with the answer: the broadcast logs it, the
/// direct responder sends it back to the node that asked.
#[derive(Debug)]
pub(crate) enum TeeAdmissionVerdict {
    /// The credential does not certify the attested key.
    ForeignCredential,
    /// The challenge is not one this node issued to the requester, or was spent
    /// or lapsed.
    ChallengeRefused,
    /// The quote or its binding did not verify.
    AttestationInvalid,
    /// Verification passed; this is what `admit_tee_node` did with it.
    Decided(TeeAdmissionOutcome),
}

impl TeeAdmissionVerdict {
    /// The requester is in: this node admitted it, or it already was a member.
    pub(crate) const fn admitted(&self) -> bool {
        matches!(
            self,
            Self::Decided(TeeAdmissionOutcome::Admitted | TeeAdmissionOutcome::AlreadyMember)
        )
    }

    /// Why the requester is not in, for its log. Empty when [`Self::admitted`].
    pub(crate) fn reason(&self) -> String {
        match self {
            Self::ForeignCredential => {
                "the account credential does not certify the attested key".to_owned()
            }
            Self::ChallengeRefused => {
                "the challenge is not one this node issued to you, or it was already used or has \
                 lapsed; ask for a new one"
                    .to_owned()
            }
            Self::AttestationInvalid => "the attestation did not verify".to_owned(),
            Self::Decided(TeeAdmissionOutcome::NotAVoucher) => {
                "this node is neither an admin nor an admitted TEE of the namespace, so it may not \
                 vouch; ask another admitter"
                    .to_owned()
            }
            Self::Decided(TeeAdmissionOutcome::Admitted | TeeAdmissionOutcome::AlreadyMember) => {
                String::new()
            }
        }
    }
}

/// Verify one TEE's attestation and, if it passes, have the context manager
/// admit it.
///
/// `Err` is reserved for the refusals `admit_tee_node` raises (policy mismatch,
/// reused quote, no policy set) and for faults. Everything that is an ordinary
/// answer comes back as a [`TeeAdmissionVerdict`]. Every `Err` comes after the
/// challenge is spent and the credential found to certify the attested key; the
/// direct responder relies on that to read the requester's account from it.
pub(crate) async fn verify_and_admit(
    context_client: &calimero_context_client::client::ContextClient,
    challenges: &TeeChallenges,
    source: libp2p::PeerId,
    group_id_bytes: [u8; 32],
    claim: TeeAdmissionClaim,
) -> eyre::Result<TeeAdmissionVerdict> {
    let TeeAdmissionClaim {
        quote_bytes,
        public_key,
        challenge,
        account,
        release_version,
    } = claim;
    let group_id = ContextGroupId::from(group_id_bytes);

    // Spent before anything else is asked of the claim, so a refused attempt
    // cannot be retried against the same challenge.
    if !challenges.consume(&challenge, group_id_bytes, source, &public_key) {
        warn!(
            %source,
            %public_key,
            "TEE admission presented a challenge this node did not issue to it, or one already \
             used or lapsed; refusing"
        );
        return Ok(TeeAdmissionVerdict::ChallengeRefused);
    }

    // The size an admission op may carry, checked before the quote is read.
    if quote_bytes.len() > calimero_governance_types::bounds::MAX_TEE_QUOTE_BYTES {
        warn!(%source, %public_key, len = quote_bytes.len(), "TEE quote is over the size bound; refusing");
        return Ok(TeeAdmissionVerdict::AttestationInvalid);
    }

    // The credential arrives unauthenticated on a gossip message, so it is
    // checked against the key the QUOTE binds to — not merely against itself.
    // The same predicate the apply path and the projection encoder use: the
    // certificate must name this key and must verify against its genesis.
    // Without it, anyone could replay another replica's credential and have the
    // verifier put it on an admission op signed with the verifier's own
    // authority.
    if !calimero_op_adapter::join_credential_certifies(&public_key, &account) {
        warn!(
            %source,
            %public_key,
            "TEE admission carried a credential that is not the attested key's; ignoring"
        );
        return Ok(TeeAdmissionVerdict::ForeignCredential);
    }

    // Without the `mock-attestation` feature there is no mock path: every quote
    // is verified with the real DCAP verifier and a `MOCK_TDX_QUOTE_V1` blob just
    // fails to parse.
    #[cfg(feature = "mock-attestation")]
    let is_mock = is_mock_quote(&quote_bytes);
    #[cfg(not(feature = "mock-attestation"))]
    let is_mock = false;

    // The quote must commit to this credential in this namespace; a fleet
    // replica is admitted to the namespace root, so the group is the namespace.
    let binding = calimero_op_adapter::tee_admission_binding(
        &group_id_bytes,
        &group_id_bytes,
        &public_key,
        &account,
    );

    // Read from the quote's own bytes first, so one that cannot match costs no
    // collateral fetch.
    let committed = calimero_tee_attestation::quote_report_data(&quote_bytes);
    if !committed.is_ok_and(|rd| rd[..32] == challenge && rd[32..] == binding) {
        warn!(%source, %public_key, "TEE quote does not carry the challenge and binding; refusing");
        return Ok(TeeAdmissionVerdict::AttestationInvalid);
    }

    #[cfg(feature = "mock-attestation")]
    let verification_result = if is_mock {
        warn!("Verifying MOCK attestation for TEE admission");
        verify_mock_attestation(&quote_bytes, &challenge, &binding)?
    } else {
        verify_attestation(&quote_bytes, &challenge, &binding).await?
    };
    #[cfg(not(feature = "mock-attestation"))]
    let verification_result = verify_attestation(&quote_bytes, &challenge, &binding).await?;

    if !verification_result.is_valid() {
        warn!(
            %source,
            quote_verified = verification_result.quote_verified,
            nonce_verified = verification_result.nonce_verified,
            "TEE attestation verification failed"
        );
        return Ok(TeeAdmissionVerdict::AttestationInvalid);
    }

    let quote_hash: [u8; 32] = Sha256::digest(&quote_bytes).into();

    // Extract measurements from the verified quote
    let mrtd = verification_result.quote.body.mrtd.clone();
    let rtmr0 = verification_result.quote.body.rtmr0.clone();
    let rtmr1 = verification_result.quote.body.rtmr1.clone();
    let rtmr2 = verification_result.quote.body.rtmr2.clone();
    let rtmr3 = verification_result.quote.body.rtmr3.clone();
    let tcb_status = verification_result
        .tcb_status
        .clone()
        .unwrap_or_else(|| "Unknown".to_owned());

    info!(
        %source, %public_key, ?group_id, %mrtd, %tcb_status, is_mock,
        quote_hash = %hex::encode(quote_hash),
        "TEE attestation verified successfully"
    );

    // Evidence for the TEE authority: the quote plus the collateral it is
    // judged against, so every peer can verify the measurements offline rather
    // than take this node's word for them. A mock quote carries none.
    let collateral = if is_mock {
        None
    } else {
        let collateral = calimero_tee_attestation::fetch_collateral(&quote_bytes).await?;
        Some(serde_json::to_vec(&collateral)?)
    };
    let attested_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();

    // Delegate policy checking and governance op publishing to the context manager.
    // The context manager has access to the store and signing keys.
    use calimero_context_client::group::AdmitTeeNodeRequest;

    context_client
        .admit_tee_node(AdmitTeeNodeRequest {
            group_id,
            member: public_key,
            account: Some(account),
            quote_hash,
            mrtd,
            rtmr0,
            rtmr1,
            rtmr2,
            rtmr3,
            tcb_status,
            is_mock,
            release_version,
            evidence: Some(
                calimero_context_client::group::TeeAuthorityEvidencePayload {
                    quote: quote_bytes,
                    collateral,
                    attested_at,
                },
            ),
        })
        .await
        .map(TeeAdmissionVerdict::Decided)
}
