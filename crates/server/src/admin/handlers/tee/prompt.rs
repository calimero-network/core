//! The prompt a TEE replica publishes on a namespace topic.
//!
//! Built in one place because two callers send it: fleet-join, to be admitted,
//! and the evidence retry, to have an admitter refresh the evidence an earlier
//! admission left out or let lapse.
//!
//! The prompt carries no quote and admits nobody. A member that may vouch
//! answers the node that published it with a challenge, and the node makes a
//! quote over that challenge for its own credential. So what this builds is the
//! prompt, plus the parameters the node keeps to answer with: nothing can be
//! attested ahead of the challenge.

use calimero_context_config::types::ContextGroupId;
use calimero_node_primitives::client::TeeAdmissionParams;
use calimero_node_primitives::sync::BroadcastMessage;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use tracing::error;

/// A prompt ready to publish, with what the node answers the challenge from.
pub(crate) struct Prompt {
    /// Registered with the node before the prompt is published, so it can answer
    /// the challenge a member offers.
    pub params: TeeAdmissionParams,
    /// The borsh `BroadcastMessage::TeeAdmissionPrompt`.
    pub payload: Vec<u8>,
}

/// Why a prompt could not be built. Each maps to the message fleet-join answers
/// with.
#[derive(Debug)]
pub(crate) enum PromptError {
    Credential,
    Serialize,
}

impl PromptError {
    pub(crate) const fn message(&self) -> &'static str {
        match self {
            Self::Credential => "could not build the account credential for this replica",
            Self::Serialize => "Failed to serialize prompt",
        }
    }
}

/// Build the prompt for `public_key` in `namespace_id`, with this replica's
/// account credential, which travels to the admitter in the node's answer.
///
/// `release_version` names the mero-tee node release this replica runs, for
/// admitters under a signed-release policy. `mock_tee` makes the node attest
/// with a mock quote, which only a build with the `mock-attestation` feature can
/// do; any other build refuses to, so a real deployment never presents one.
pub(crate) fn build(
    store: &Store,
    namespace_id: &ContextGroupId,
    public_key: PublicKey,
    release_version: Option<&str>,
    admitter_addrs: Vec<String>,
    mock_tee: bool,
) -> Result<Prompt, PromptError> {
    // The credential is ours, so it travels with the node's answer; built locally,
    // which is why a replica can produce one before it holds any scope key.
    let account = calimero_context::join_credential::build(store, namespace_id, &public_key)
        .map_err(|err| {
            error!(error=?err, "could not build this replica's account credential");
            PromptError::Credential
        })?;

    let payload = borsh::to_vec(&BroadcastMessage::TeeAdmissionPrompt).map_err(|err| {
        error!(error=?err, "Failed to serialize TeeAdmissionPrompt");
        PromptError::Serialize
    })?;

    Ok(Prompt {
        params: TeeAdmissionParams {
            namespace_id: namespace_id.to_bytes(),
            admitter_addrs,
            public_key,
            account,
            release_version: release_version.map(str::to_owned),
            mock_tee,
        },
        payload,
    })
}
