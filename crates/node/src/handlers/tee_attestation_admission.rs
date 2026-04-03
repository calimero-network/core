//! TEE attestation-based admission handler.
//!
//! When a fleet TEE node broadcasts `TeeAttestationAnnounce` on a group topic,
//! existing peers verify the TDX quote against the group's `TeeAdmissionPolicy`
//! and, if valid, admit the node via a `MemberJoinedViaTeeAttestation` governance op.
//!
//! The heavy lifting (policy lookup, governance op signing, DAG interaction) is
//! delegated to `calimero_context::group_store` via the `ContextClient`.

use calimero_context_config::types::ContextGroupId;
use calimero_primitives::identity::PublicKey;
use calimero_tee_attestation::{is_mock_quote, verify_attestation, verify_mock_attestation};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

/// Handle a `TeeAttestationAnnounce` broadcast on a group gossip topic.
///
/// Verifies the TDX quote, checks measurements against the group's TEE admission
/// policy, and publishes a `MemberJoinedViaTeeAttestation` governance op if valid.
pub async fn handle_tee_attestation_announce(
    context_client: &calimero_context_client::client::ContextClient,
    source: libp2p::PeerId,
    quote_bytes: Vec<u8>,
    public_key: PublicKey,
    nonce: [u8; 32],
    group_id_bytes: [u8; 32],
) -> eyre::Result<()> {
    let group_id = ContextGroupId::from(group_id_bytes);

    let is_mock = is_mock_quote(&quote_bytes);

    let pk_hash: [u8; 32] = Sha256::digest(*public_key).into();

    let verification_result = if is_mock {
        warn!("Verifying MOCK attestation for TEE admission");
        verify_mock_attestation(&quote_bytes, &nonce, Some(&pk_hash))?
    } else {
        verify_attestation(&quote_bytes, &nonce, Some(&pk_hash)).await?
    };

    if !verification_result.is_valid() {
        warn!(
            %source,
            quote_verified = verification_result.quote_verified,
            nonce_verified = verification_result.nonce_verified,
            "TEE attestation verification failed"
        );
        return Ok(());
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

    // Delegate policy checking and governance op publishing to the context manager.
    // The context manager has access to the store and signing keys.
    use calimero_context_client::group::AdmitTeeNodeRequest;

    context_client
        .admit_tee_node(AdmitTeeNodeRequest {
            group_id,
            member: public_key,
            quote_hash,
            mrtd,
            rtmr0,
            rtmr1,
            rtmr2,
            rtmr3,
            tcb_status,
            is_mock,
        })
        .await
}
