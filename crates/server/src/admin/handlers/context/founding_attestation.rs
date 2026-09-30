//! After founding a namespace for a member, a TEE relay admits itself as the
//! namespace's first TEE (`GroupOp::FoundingRelayAttested`).
//!
//! The founder has no node, so nobody else could ever admit a TEE here. The
//! relay quotes over its identity in the namespace, attaches the collateral so
//! every peer can verify the quote offline, reads which profile of its signed
//! release its measurements match (as any admitter would), and publishes. Peers
//! verify the quote themselves; see the op's apply for what they check.

use calimero_context_config::types::ContextGroupId;
use calimero_primitives::identity::PublicKey;
use calimero_tee_attestation::{build_report_data, generate_attestation};
use sha2::{Digest, Sha256};

use crate::AdminState;

/// `Ok(true)` once admitted; `Ok(false)` on a relay that is not a TEE.
///
/// # Errors
/// Why the attestation could not be made or was refused.
pub(super) async fn attest(
    state: &AdminState,
    namespace_id: &ContextGroupId,
    identity: PublicKey,
) -> Result<bool, String> {
    #[cfg(feature = "mock-attestation")]
    let mock_tee = state.mock_tee;
    #[cfg(not(feature = "mock-attestation"))]
    let mock_tee = false;
    let Some(release_version) = state.tee_release_version.clone().or_else(|| {
        // A mock rig has no release; it still exercises the admission.
        mock_tee.then(|| "mock".to_owned())
    }) else {
        return Ok(false);
    };

    let key_hash: [u8; 32] = Sha256::digest(*identity).into();
    let nonce: [u8; 32] = rand::random();
    let report_data = build_report_data(&nonce, Some(&key_hash));
    #[cfg(feature = "mock-attestation")]
    let attestation = if mock_tee {
        Ok(calimero_tee_attestation::generate_mock_attestation(
            report_data,
        ))
    } else {
        generate_attestation(report_data)
    };
    #[cfg(not(feature = "mock-attestation"))]
    let attestation = generate_attestation(report_data);
    let attestation = attestation.map_err(|err| format!("could not generate a quote: {err}"))?;
    if attestation.is_mock && !mock_tee {
        return Err("this node produced a mock quote but is not a mock TEE".to_owned());
    }

    let collateral = if attestation.is_mock {
        None
    } else {
        let value = crate::admin::handlers::tee::collateral::for_quote(&attestation.quote_bytes)
            .await
            .map_err(|err| format!("could not fetch collateral for the quote: {err}"))?;
        Some(value)
    };
    let attested_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|err| err.to_string())?
        .as_secs();
    let parsed = collateral
        .as_ref()
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()
        .map_err(|err| format!("collateral does not decode: {err}"))?;
    // Checked here so a quote this node could not stand behind fails at its own
    // API, not on every peer; the handler reads the release profile from it.
    let _verdict = calimero_tee_attestation::verify_evidence(
        &attestation.quote_bytes,
        parsed.as_ref(),
        attested_at,
        &key_hash,
    )
    .map_err(|err| format!("this node's own quote does not verify: {err}"))?;

    let account = calimero_context::join_credential::build(&state.store, namespace_id, &identity)
        .map_err(|err| format!("could not build this node's credential: {err}"))?;
    let collateral = collateral
        .map(|value| serde_json::to_vec(&value))
        .transpose()
        .map_err(|err| err.to_string())?;
    state
        .ctx_client
        .attest_founding_relay(calimero_context_client::group::AttestFoundingRelayRequest {
            namespace_id: *namespace_id,
            account,
            evidence: calimero_context_client::group::TeeAuthorityEvidencePayload {
                quote: attestation.quote_bytes,
                collateral,
                attested_at,
            },
            release_version,
        })
        .await
        .map_err(|err| format!("{err:#}"))?;
    Ok(true)
}
