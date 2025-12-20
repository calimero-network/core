//! Specialized Node Invite protocol handler
//!
//! This module handles the specialized node invitation protocol for nodes like
//! read-only TEE nodes.
//!
//! ## Protocol Flow
//!
//! ### Standard Node (initiator)
//! 1. User broadcasts `SpecializedNodeDiscovery` with nonce and node_type to global topic
//! 2. Stores nonce -> {context_id, inviter_id, state: Pending} in `PendingSpecializedNodeInvites`
//! 3. Receives `VerificationRequest` from specialized node (contains nonce, not context_id)
//! 4. Atomically transitions state to AwaitingConfirmation (prevents race conditions)
//! 5. Verifies the node (e.g., TEE attestation)
//! 6. If valid, creates regular invitation and sends `SpecializedNodeInvitationResponse`
//! 7. Waits for `SpecializedNodeJoinConfirmation` on context topic
//! 8. If confirmation received, removes pending entry
//! 9. If TTL expires (60s) without confirmation, resets to Pending for retry
//!
//! ### Specialized Node (e.g., Read-Only TEE Node)
//! 1. Receives `SpecializedNodeDiscovery` broadcast (subscribed to global topic)
//! 2. Generates verification data (e.g., TEE attestation with nonce)
//! 3. Sends `VerificationRequest` via request-response (no context_id needed)
//! 4. Receives `SpecializedNodeInvitationResponse` with ContextInvitationPayload
//! 5. Joins context using the invitation payload
//! 6. Broadcasts `SpecializedNodeJoinConfirmation` on context topic

use crate::specialized_node_invite_state::{
    InviteState, PendingSpecializedNodeInvites, SpecializedNodeInviteAction,
};
use calimero_context_primitives::client::ContextClient;
use calimero_network_primitives::specialized_node_invite::{
    SpecializedNodeInvitationResponse, VerificationRequest,
};
use calimero_primitives::context::{ContextId, ContextInvitationPayload};
use calimero_primitives::identity::PublicKey;
use calimero_tee_attestation::{
    build_report_data, generate_attestation, is_mock_quote, verify_attestation,
    verify_mock_attestation,
};
use libp2p::PeerId;
use tracing::{debug, error, info, warn};

/// Handle a specialized node discovery broadcast (for specialized nodes in read-only mode)
///
/// When a specialized node receives this broadcast, it:
/// 1. Creates a new identity using `context_client.new_identity()` (stored in datastore)
/// 2. Generates verification data (e.g., TEE attestation with report_data = nonce)
/// 3. Returns request to send to the source peer with verification data + public key
///
/// The private key is securely stored in the datastore under `ContextId::zero()` (identity pool).
/// When joining the context, `join_context` retrieves the identity from the pool automatically.
pub fn handle_specialized_node_discovery(
    nonce: [u8; 32],
    source_peer: PeerId,
    context_client: &ContextClient,
) -> eyre::Result<VerificationRequest> {
    info!(
        %source_peer,
        nonce = %hex::encode(nonce),
        "Received specialized node discovery - generating verification"
    );

    // Create a new identity - this generates a keypair and stores the private key
    // in the datastore under ContextId::zero() (identity pool)
    let our_public_key = context_client.new_identity()?;

    info!(
        public_key = %our_public_key,
        "Created identity for specialized node invitation (private key stored in datastore)"
    );

    // Build report_data: nonce || zeros
    // The nonce alone provides replay protection - context_id is tracked by the requester
    let report_data = build_report_data(&nonce, None);

    // Generate attestation
    let attestation_result = generate_attestation(report_data)?;

    info!(
        quote_len = attestation_result.quote_bytes.len(),
        "TEE attestation generated successfully for specialized node verification"
    );

    // Create the verification request to send to the inviting node
    let request = VerificationRequest::TeeAttestation {
        nonce,
        quote_bytes: attestation_result.quote_bytes,
        public_key: our_public_key,
    };

    Ok(request)
}

/// Handle receiving a verification request (for standard/inviting nodes)
///
/// When an inviting node receives this request, it:
/// 1. Atomically claims the nonce by transitioning state to AwaitingConfirmation
/// 2. Verifies the specialized node (e.g., TEE attestation, supports mock for testing)
/// 3. If valid, creates a regular invitation for the node's public key
/// 4. Sends the invitation back via the response channel
///
/// State machine prevents race conditions:
/// - Only one request can claim a Pending nonce at a time
/// - If AwaitingConfirmation, check TTL before allowing retry
/// - On failure, state is reset to Pending for retry
pub async fn handle_verification_request(
    peer_id: PeerId,
    request: VerificationRequest,
    pending_invites: &PendingSpecializedNodeInvites,
    context_client: &ContextClient,
    accept_mock_tee: bool,
) -> SpecializedNodeInvitationResponse {
    let nonce = *request.nonce();
    let public_key = *request.public_key();

    info!(
        %peer_id,
        public_key = %public_key,
        nonce = %hex::encode(nonce),
        "Received verification request - verifying specialized node"
    );

    // Atomically check and claim the pending invite.
    // This prevents race conditions where multiple specialized nodes could
    // all get invitations from a single broadcast.
    let (context_id, inviter_id) = {
        let mut entry = match pending_invites.get_mut(&nonce) {
            Some(entry) => entry,
            None => {
                warn!(
                    nonce = %hex::encode(nonce),
                    "Received verification request for unknown nonce"
                );
                return SpecializedNodeInvitationResponse::error(
                    nonce,
                    "Unknown nonce - no pending invite request",
                );
            }
        };

        // Check if we can accept this request based on current state
        if !entry.state.can_accept_request() {
            // Already processing another request and TTL hasn't expired
            if let InviteState::AwaitingConfirmation {
                invitee_public_key, ..
            } = &entry.state
            {
                warn!(
                    nonce = %hex::encode(nonce),
                    current_invitee = %invitee_public_key,
                    new_requester = %public_key,
                    "Nonce already claimed by another specialized node, TTL not expired"
                );
            }
            return SpecializedNodeInvitationResponse::error(
                nonce,
                "Invite already in progress - please wait for TTL expiry",
            );
        }

        // Extract context_id and inviter_id before transitioning state
        let (context_id, inviter_id) = match &entry.action {
            SpecializedNodeInviteAction::HandleContextInvite {
                context_id,
                inviter_id,
            } => (*context_id, *inviter_id),
        };

        // Atomically transition to AwaitingConfirmation to claim this nonce
        entry.transition_to_awaiting(public_key);

        info!(
            %peer_id,
            %context_id,
            %inviter_id,
            "Claimed pending invite for nonce, transitioning to AwaitingConfirmation"
        );

        (context_id, inviter_id)
    }; // Release the lock here

    // Handle verification based on request type
    match request {
        VerificationRequest::TeeAttestation {
            nonce,
            quote_bytes,
            public_key,
        } => {
            // Verify the attestation - detect mock vs real quotes
            let is_mock = is_mock_quote(&quote_bytes);

            // Reject mock attestation if not allowed
            if is_mock && !accept_mock_tee {
                warn!("Received mock TEE attestation but accept_mock_tee is disabled");
                // Reset state to allow retry
                reset_to_pending(pending_invites, &nonce);
                return SpecializedNodeInvitationResponse::error(
                    nonce,
                    "Mock TEE attestation not accepted in this environment",
                );
            }

            let verification_result = if is_mock {
                // Mock attestation for development/testing
                warn!("Verifying MOCK attestation - NOT FOR PRODUCTION USE");
                match verify_mock_attestation(
                    &quote_bytes,
                    &nonce,
                    None, // No app hash expected
                ) {
                    Ok(result) => result,
                    Err(err) => {
                        error!(error = %err, "Failed to verify mock TEE attestation");
                        // Reset state to allow retry
                        reset_to_pending(pending_invites, &nonce);
                        return SpecializedNodeInvitationResponse::error(
                            nonce,
                            format!("Mock attestation verification failed: {}", err),
                        );
                    }
                }
            } else {
                // Real TDX attestation
                match verify_attestation(
                    &quote_bytes,
                    &nonce,
                    None, // No app hash expected - specialized node doesn't know context_id
                )
                .await
                {
                    Ok(result) => result,
                    Err(err) => {
                        error!(error = %err, "Failed to verify TEE attestation");
                        // Reset state to allow retry
                        reset_to_pending(pending_invites, &nonce);
                        return SpecializedNodeInvitationResponse::error(
                            nonce,
                            format!("Attestation verification failed: {}", err),
                        );
                    }
                }
            };

            if !verification_result.is_valid() {
                warn!(
                    quote_verified = verification_result.quote_verified,
                    nonce_verified = verification_result.nonce_verified,
                    app_hash_verified = ?verification_result.application_hash_verified,
                    is_mock = is_mock,
                    "TEE attestation verification failed"
                );
                // Reset state to allow retry
                reset_to_pending(pending_invites, &nonce);
                return SpecializedNodeInvitationResponse::error(
                    nonce,
                    "Attestation verification failed",
                );
            }

            info!(
                %peer_id,
                %context_id,
                %public_key,
                is_mock = is_mock,
                "TEE attestation verified successfully"
            );

            // Create invitation for the verified node
            let response = create_invitation_response(
                nonce,
                context_client,
                context_id,
                inviter_id,
                public_key,
            )
            .await;

            // If invitation creation failed, reset state to allow retry
            if response.invitation_bytes.is_none() {
                reset_to_pending(pending_invites, &nonce);
            }
            // Note: We don't remove the entry here - it stays in AwaitingConfirmation
            // until we receive a join confirmation or TTL expires

            response
        }
    }
}

/// Reset a pending invite back to Pending state to allow retry
fn reset_to_pending(pending_invites: &PendingSpecializedNodeInvites, nonce: &[u8; 32]) {
    if let Some(mut entry) = pending_invites.get_mut(nonce) {
        debug!(
            nonce = %hex::encode(nonce),
            "Resetting invite state to Pending for retry"
        );
        entry.reset_to_pending();
    }
}

/// Handle a join confirmation from a specialized node
///
/// Called when a specialized node broadcasts `SpecializedNodeJoinConfirmation`
/// on the context topic after successfully joining.
pub fn handle_join_confirmation(pending_invites: &PendingSpecializedNodeInvites, nonce: [u8; 32]) {
    if let Some((_, pending)) = pending_invites.remove(&nonce) {
        let context_id = match &pending.action {
            SpecializedNodeInviteAction::HandleContextInvite { context_id, .. } => context_id,
        };
        info!(
            nonce = %hex::encode(nonce),
            %context_id,
            "Received join confirmation - specialized node successfully joined, removing pending invite"
        );
    } else {
        debug!(
            nonce = %hex::encode(nonce),
            "Received join confirmation for unknown nonce (already removed or never existed)"
        );
    }
}

/// Create an invitation for a verified specialized node
async fn create_invitation_response(
    nonce: [u8; 32],
    context_client: &ContextClient,
    context_id: ContextId,
    inviter_id: PublicKey,
    invitee_public_key: PublicKey,
) -> SpecializedNodeInvitationResponse {
    // Create a regular invitation for the specialized node's public key
    // This adds the node's public key as a member on-chain
    let invitation_payload = match context_client
        .invite_member(&context_id, &inviter_id, &invitee_public_key)
        .await
    {
        Ok(Some(payload)) => payload,
        Ok(None) => {
            error!(%context_id, "Context configuration not found");
            return SpecializedNodeInvitationResponse::error(
                nonce,
                "Context configuration not found",
            );
        }
        Err(err) => {
            error!(error = %err, %context_id, "Failed to create invitation for specialized node");
            return SpecializedNodeInvitationResponse::error(
                nonce,
                format!("Failed to create invitation: {}", err),
            );
        }
    };

    info!(
        %context_id,
        %invitee_public_key,
        "Created invitation for specialized node"
    );

    // Serialize the invitation payload
    let invitation_bytes = invitation_payload.to_string().into_bytes();

    SpecializedNodeInvitationResponse::success(nonce, invitation_bytes)
}

/// Handle receiving a specialized node invitation response (for specialized nodes)
///
/// When a specialized node receives this response, it:
/// 1. Checks for errors in the response
/// 2. If successful, deserializes the ContextInvitationPayload
/// 3. Joins the context using the invitation
/// 4. Returns the nonce and context_id for confirmation broadcast
pub async fn handle_specialized_node_invitation_response(
    peer_id: PeerId,
    nonce: [u8; 32],
    response: SpecializedNodeInvitationResponse,
    context_client: &ContextClient,
) -> eyre::Result<Option<ContextId>> {
    if let Some(error) = &response.error {
        warn!(
            %peer_id,
            %error,
            "Specialized node invitation request was rejected"
        );
        return Ok(None);
    }

    let Some(invitation_bytes) = response.invitation_bytes else {
        error!(%peer_id, "Specialized node invitation response missing both invitation and error");
        return Ok(None);
    };

    info!(
        %peer_id,
        nonce = %hex::encode(nonce),
        invitation_len = invitation_bytes.len(),
        "Received specialized node invitation - joining context"
    );

    // Deserialize ContextInvitationPayload from the bytes
    // The payload is serialized as a Base58-encoded string
    let invitation_str = match String::from_utf8(invitation_bytes) {
        Ok(s) => s,
        Err(err) => {
            error!(%peer_id, error = %err, "Failed to decode invitation bytes as UTF-8");
            return Ok(None);
        }
    };

    let invitation_payload: ContextInvitationPayload = match invitation_str.parse() {
        Ok(payload) => payload,
        Err(err) => {
            error!(%peer_id, error = %err, "Failed to parse ContextInvitationPayload");
            return Ok(None);
        }
    };

    info!(
        %peer_id,
        invitation_len = invitation_str.len(),
        "Joining context via specialized node invitation"
    );

    // Join the context using the invitation payload
    match context_client.join_context(invitation_payload).await {
        Ok(join_response) => {
            info!(
                %peer_id,
                context_id = %join_response.context_id,
                member_public_key = %join_response.member_public_key,
                "Successfully joined context via specialized node invitation"
            );
            // Return the context_id so caller can broadcast confirmation
            Ok(Some(join_response.context_id))
        }
        Err(err) => {
            error!(
                %peer_id,
                error = %err,
                "Failed to join context via specialized node invitation"
            );
            Ok(None)
        }
    }
}
