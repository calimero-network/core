//! TEE attestation-based admission handler.
//!
//! When a fleet TEE node broadcasts `TeeAttestationAnnounce` on the namespace
//! governance topic (`ns/<hex>`; the namespace is its own root group),
//! existing peers verify the TDX quote against the group's `TeeAdmissionPolicy`
//! and, if valid, admit the node via a `MemberJoinedViaTeeAttestation` governance op.
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

/// Compute the application-hash binding that ties a TEE attestation to a node
/// identity.
///
/// The attestation generator and every verifier must hash the public key the
/// exact same way, otherwise the mandatory app-hash binding check fails. The
/// `**public_key` below derefs twice — `&PublicKey` -> `PublicKey`, then the
/// `PublicKey` newtype (over `[u8; 32]`) -> its raw 32-byte ed25519 key material —
/// so the hash is taken over the canonical key bytes. Centralizing it here keeps
/// the generation and verification sides from silently diverging if `PublicKey`'s
/// representation ever changes.
pub(crate) fn public_key_binding_hash(public_key: &PublicKey) -> [u8; 32] {
    Sha256::digest(**public_key).into()
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
    /// The quote or its nonce binding did not verify.
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

/// Handle a `TeeAttestationAnnounce` broadcast on a namespace gossip topic.
///
/// Verifies the TDX quote, checks measurements against the group's TEE admission
/// policy, and publishes a `MemberJoinedViaTeeAttestation` governance op if valid.
/// Nobody is waiting on the answer, so it is logged and dropped.
pub async fn handle_tee_attestation_announce(
    context_client: &calimero_context_client::client::ContextClient,
    source: libp2p::PeerId,
    quote_bytes: Vec<u8>,
    public_key: PublicKey,
    nonce: [u8; 32],
    group_id_bytes: [u8; 32],
    account: Box<calimero_context_client::local_governance::JoinAccountCredential>,
) -> eyre::Result<()> {
    let verdict = verify_and_admit(
        context_client,
        source,
        quote_bytes,
        public_key,
        nonce,
        group_id_bytes,
        account,
    )
    .await?;
    tracing::debug!(%source, %public_key, ?verdict, "TEE attestation announce handled");
    Ok(())
}

/// Verify one TEE's attestation and, if it passes, have the context manager
/// admit it.
///
/// `Err` is reserved for the refusals `admit_tee_node` raises (policy mismatch,
/// reused quote, no policy set) and for faults. Everything that is an ordinary
/// answer comes back as a [`TeeAdmissionVerdict`].
pub(crate) async fn verify_and_admit(
    context_client: &calimero_context_client::client::ContextClient,
    source: libp2p::PeerId,
    quote_bytes: Vec<u8>,
    public_key: PublicKey,
    nonce: [u8; 32],
    group_id_bytes: [u8; 32],
    account: Box<calimero_context_client::local_governance::JoinAccountCredential>,
) -> eyre::Result<TeeAdmissionVerdict> {
    let group_id = ContextGroupId::from(group_id_bytes);

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
            "TEE announcement carried a credential that is not the attested key's; ignoring"
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

    let pk_hash = public_key_binding_hash(&public_key);

    #[cfg(feature = "mock-attestation")]
    let verification_result = if is_mock {
        warn!("Verifying MOCK attestation for TEE admission");
        verify_mock_attestation(&quote_bytes, &nonce, &pk_hash)?
    } else {
        verify_attestation(&quote_bytes, &nonce, &pk_hash).await?
    };
    #[cfg(not(feature = "mock-attestation"))]
    let verification_result = verify_attestation(&quote_bytes, &nonce, &pk_hash).await?;

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
