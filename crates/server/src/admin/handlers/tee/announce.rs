//! The attestation announcement a TEE replica publishes on a namespace topic.
//!
//! Built in one place because two callers send it: fleet-join, to be admitted,
//! and the evidence retry, to have an admitter publish the evidence an earlier
//! admission left out.

use calimero_context_client::local_governance::JoinAccountCredential;
use calimero_context_config::types::ContextGroupId;
use calimero_network_primitives::specialized_node_invite::SpecializedNodeType;
use calimero_node_primitives::sync::BroadcastMessage;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
#[cfg(feature = "mock-attestation")]
use calimero_tee_attestation::generate_mock_attestation;
use calimero_tee_attestation::{build_report_data, generate_attestation};
use sha2::{Digest, Sha256};
use tracing::error;

/// A signed-off announcement, with the parts fleet-join also sends directly.
pub(crate) struct Announcement {
    pub quote_bytes: Vec<u8>,
    pub nonce: [u8; 32],
    pub account: Box<JoinAccountCredential>,
    /// The borsh announcements, ready to publish: the
    /// `TeeReleaseAttestationAnnounce` first when the release is known, then
    /// the `TeeAttestationAnnounce` every admitter decodes, including those
    /// that predate the release form.
    pub payloads: Vec<Vec<u8>>,
}

/// Why an announcement could not be built. Each maps to the message fleet-join
/// has always answered with.
#[derive(Debug)]
pub(crate) enum AnnounceError {
    Attestation,
    MockRejected,
    Credential,
    Serialize,
}

impl AnnounceError {
    pub(crate) const fn message(&self) -> &'static str {
        match self {
            Self::Attestation => "Failed to generate attestation",
            Self::MockRejected => "TDX attestation required -- mock not accepted for fleet join",
            Self::Credential => "could not build the account credential for this replica",
            Self::Serialize => "Failed to serialize announcement",
        }
    }
}

/// Attest `public_key` with a fresh nonce and wrap it, with this replica's
/// account credential, as a `TeeAttestationAnnounce` -- and, when
/// `release_version` names the mero-tee node release this replica runs, as a
/// `TeeReleaseAttestationAnnounce` too, for admitters under a signed-release
/// policy.
///
/// `mock_tee` produces and accepts a mock quote; without it any mock result is
/// refused, so a real deployment never announces one.
pub(crate) fn build(
    store: &Store,
    namespace_id: &ContextGroupId,
    public_key: PublicKey,
    release_version: Option<&str>,
    #[cfg(feature = "mock-attestation")] mock_tee: bool,
) -> Result<Announcement, AnnounceError> {
    let pk_hash: [u8; 32] = Sha256::digest(*public_key).into();
    let nonce: [u8; 32] = rand::random();
    let report_data = build_report_data(&nonce, Some(&pk_hash));

    // Under --mock-tee, deliberately produce a mock quote (any OS, no TDX
    // hardware) and accept it below. The real path generates a hardware
    // attestation and still rejects any mock result.
    #[cfg(feature = "mock-attestation")]
    let attestation = if mock_tee {
        Ok(generate_mock_attestation(report_data))
    } else {
        generate_attestation(report_data)
    };
    #[cfg(not(feature = "mock-attestation"))]
    let attestation = generate_attestation(report_data);
    let attestation = attestation.map_err(|err| {
        error!(error=?err, "Failed to generate TDX attestation");
        AnnounceError::Attestation
    })?;

    #[cfg(feature = "mock-attestation")]
    let reject_mock = attestation.is_mock && !mock_tee;
    #[cfg(not(feature = "mock-attestation"))]
    let reject_mock = attestation.is_mock;
    if reject_mock {
        error!("Mock attestation generated -- a TEE announcement requires real TDX hardware");
        return Err(AnnounceError::MockRejected);
    }

    // The verifier publishes the admission op, but the credential is ours — so
    // it has to travel with the announcement. Built locally: `ensure_enrolled`
    // mints the device row without publishing or encrypting anything, which is
    // exactly why a replica can produce one before it holds any scope key.
    let account = calimero_context::join_credential::build(store, namespace_id, &public_key)
        .map_err(|err| {
            error!(error=?err, "could not build this replica's account credential");
            AnnounceError::Credential
        })?;

    let mut messages = Vec::with_capacity(2);
    if let Some(release_version) = release_version {
        messages.push(BroadcastMessage::TeeReleaseAttestationAnnounce {
            quote_bytes: attestation.quote_bytes.clone(),
            public_key,
            nonce,
            node_type: SpecializedNodeType::ReadOnly,
            account: account.clone(),
            release_version: release_version.to_owned(),
        });
    }
    messages.push(BroadcastMessage::TeeAttestationAnnounce {
        quote_bytes: attestation.quote_bytes.clone(),
        public_key,
        nonce,
        node_type: SpecializedNodeType::ReadOnly,
        account: account.clone(),
    });
    let payloads = messages
        .iter()
        .map(borsh::to_vec)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| {
            error!(error=?err, "Failed to serialize TeeAttestationAnnounce");
            AnnounceError::Serialize
        })?;

    Ok(Announcement {
        quote_bytes: attestation.quote_bytes,
        nonce,
        account,
        payloads,
    })
}
