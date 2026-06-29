//! Blob protocol stream handling
//!
//! **SRP**: This module handles the blob protocol for P2P blob transfer
//! Implements chunked blob streaming with flow control and timeouts

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use calimero_context_client::client::{ContextClient, ContextRegistry};
use calimero_network_primitives::{
    blob_types::{BlobAuthPayload, BlobChunk, BlobRequest, BlobResponse},
    stream::{Message as StreamMessage, Stream},
};
use calimero_node_primitives::client::NodeClient;
use futures_util::{SinkExt, StreamExt};
use libp2p::PeerId;
use tokio::time::timeout;
use tracing::{debug, error, info, warn};

// Timeout settings for blob serving
const BLOB_SERVE_TIMEOUT: Duration = Duration::from_secs(300); // 5 minutes total
const CHUNK_SEND_TIMEOUT: Duration = Duration::from_secs(30); // 30 seconds per chunk

// Replay protection window (30 seconds past, 10 seconds future)
const MAX_REQUEST_AGE_SECS: u64 = 30;
const MAX_REQUEST_FUTURE_AGE_SECS: u64 = 10;

/// Handles streams that arrived on the blob protocol
///
/// Reads the first message as a BlobRequest, then delegates to the chunked handler.
pub async fn handle_blob_protocol_stream(
    node_client: NodeClient,
    context_client: ContextClient,
    peer_id: PeerId,
    mut stream: Box<Stream>,
) -> eyre::Result<()> {
    info!(%peer_id, "Starting blob protocol stream handler");

    // Read the first message which should be a blob request
    let first_message = match stream.next().await {
        Some(Ok(msg)) => msg,
        Some(Err(e)) => {
            debug!(%peer_id, error = %e, "Error reading blob request from stream");
            return Err(e.into());
        }
        None => {
            debug!(%peer_id, "Blob protocol stream closed immediately");
            return Ok(());
        }
    };

    // Parse as blob request
    let blob_request = serde_json::from_slice::<BlobRequest>(&first_message.data)
        .map_err(|e| eyre::eyre!("Failed to parse blob request: {}", e))?;

    if !is_blob_access_authorized(&context_client, &blob_request).await? {
        let response = BlobResponse {
            found: false,
            size: None,
        };
        let response_data = serde_json::to_vec(&response)?;

        timeout(
            CHUNK_SEND_TIMEOUT,
            stream.send(StreamMessage::new(response_data)),
        )
        .await
        .map_err(|_| eyre::eyre!("Timeout sending auth rejection"))??;

        return Ok(());
    }

    // Delegate to the chunked handler
    handle_blob_request_stream(node_client, peer_id, blob_request, stream).await
}

/// Handles blob requests that come over streams
///
/// This implements the chunked blob transfer protocol:
/// 1. Send BlobResponse (found/not found + size)
/// 2. If found, stream blob chunks
/// 3. Send empty chunk to signal end
///
/// Features:
/// - Timeouts (5 min total, 30 sec per chunk)
/// - Binary chunk encoding for efficiency
async fn handle_blob_request_stream(
    node_client: NodeClient,
    peer_id: PeerId,
    blob_request: BlobRequest,
    mut stream: Box<Stream>,
) -> eyre::Result<()> {
    info!(
        %peer_id,
        blob_id = %blob_request.blob_id,
        context_id = %blob_request.context_id,
        "Processing blob request stream using binary chunk protocol"
    );

    // Wrap the entire blob serving in a timeout
    let serve_result = timeout(BLOB_SERVE_TIMEOUT, async {
        // Try to get the blob as a stream (handles chunked blobs efficiently)
        info!(%peer_id, blob_id = %blob_request.blob_id, "Attempting to get blob from local storage");
        let blob_stream = node_client.get_blob(&blob_request.blob_id, None).await?;

        let (response, blob_stream) = if let Some(blob_stream) = blob_stream {
            info!(%peer_id, "Blob found, will stream chunks");

            // Get blob metadata to determine size
            let blob_metadata = node_client.get_blob_info(blob_request.blob_id).await?;

            let total_size = blob_metadata.map(|meta| meta.size).unwrap_or(0);

            let response = BlobResponse {
                found: true,
                size: Some(total_size),
            };

            (response, Some(blob_stream))
        } else {
            info!(%peer_id, "Blob not found");
            let response = BlobResponse {
                found: false,
                size: None,
            };
            (response, None)
        };

        // Send initial response with timeout
        let response_data = serde_json::to_vec(&response)
            .map_err(|e| eyre::eyre!("Failed to serialize blob response: {}", e))?;

        timeout(
            CHUNK_SEND_TIMEOUT,
            stream.send(StreamMessage::new(response_data)),
        )
        .await
        .map_err(|_| eyre::eyre!("Timeout sending response"))?
        .map_err(|e| eyre::eyre!("Failed to send blob response: {}", e))?;

        // If blob was found, stream the chunks
        if response.found {
            let mut blob_stream = blob_stream.expect("Blob stream should exist since response.found is true");

            debug!(%peer_id, "Starting to stream blob chunks");

            let mut chunk_count = 0;
            let mut total_bytes_sent = 0;

            while let Some(chunk_result) = blob_stream.next().await {
                match chunk_result {
                    Ok(chunk) => {
                        chunk_count += 1;
                        total_bytes_sent += chunk.len();

                        debug!(
                            %peer_id,
                            chunk_number = chunk_count,
                            chunk_size = chunk.len(),
                            total_sent = total_bytes_sent,
                            "Sending blob chunk"
                        );

                        let blob_chunk = BlobChunk {
                            data: chunk.to_vec(),
                        };

                        let chunk_data = borsh::to_vec(&blob_chunk)
                            .map_err(|e| eyre::eyre!("Failed to serialize blob chunk: {}", e))?;

                        debug!(
                            %peer_id,
                            chunk_number = chunk_count,
                            original_chunk_size = chunk.len(),
                            binary_message_size = chunk_data.len(),
                            "Sending binary chunk data"
                        );

                        // Send chunk with timeout
                        timeout(
                            CHUNK_SEND_TIMEOUT,
                            stream.send(StreamMessage::new(chunk_data)),
                        )
                        .await
                        .map_err(|_| eyre::eyre!("Timeout sending chunk {}", chunk_count))?
                        .map_err(|e| eyre::eyre!("Failed to send blob chunk: {}", e))?;
                    }
                    Err(e) => {
                        warn!(%peer_id, error = %e, "Failed to read blob chunk");
                        return Err(eyre::eyre!("Failed to read blob chunk: {}", e));
                    }
                }
            }

            // Send final empty chunk to signal end of stream
            let final_chunk = BlobChunk {
                data: Vec::new(),
            };

            let final_chunk_data = borsh::to_vec(&final_chunk)
                .map_err(|e| eyre::eyre!("Failed to serialize final chunk: {}", e))?;

            timeout(
                CHUNK_SEND_TIMEOUT,
                stream.send(StreamMessage::new(final_chunk_data)),
            )
            .await
            .map_err(|_| eyre::eyre!("Timeout sending final chunk"))?
            .map_err(|e| eyre::eyre!("Failed to send final blob chunk: {}", e))?;

            debug!(
                %peer_id,
                total_chunks = chunk_count + 1, // +1 for final chunk
                total_bytes = total_bytes_sent,
                "Successfully streamed all blob chunks"
            );
        }

        debug!(%peer_id, "Blob request stream handled successfully");
        Ok(())
    })
    .await;

    // Handle timeout result
    match serve_result {
        Ok(result) => result,
        Err(_) => {
            warn!(
                %peer_id,
                blob_id = %blob_request.blob_id,
                timeout_secs = BLOB_SERVE_TIMEOUT.as_secs(),
                "Blob serving timed out"
            );
            Err(eyre::eyre!("Blob serving timed out"))
        }
    }
}

/// Helper function to check if the blob access is authorized.
///
////// Helper function to authorize blob access.
///
/// Implements the security policy:
/// 1. Public blobs (App Bundles) are accessible to everyone (bootstrapping).
/// 2. Private blobs require a valid signature from a Context Member.
///
/// # Returns
/// * `Ok(true)` - if access is granted.
/// * `Ok(false)` - if access is denied.
/// * `Err` - only on internal system failures (e.g. DB errors).
async fn is_blob_access_authorized(
    context_client: &ContextClient,
    request: &BlobRequest,
) -> eyre::Result<bool> {
    // Fetch Context Config
    // If we don't have the context config, we can't verify anything. Deny access.
    match context_client.context_config(&request.context_id) {
        Ok(Some(_)) => {}
        Ok(None) => {
            warn!(context_id=%request.context_id, "Context config not found locally. Denying blob access.");
            return Ok(false);
        }
        Err(e) => return Err(e),
    }

    // Check if the Blob is Public (The Application Bundle)
    // New nodes need this to join, so they cannot sign yet.
    // We identify if the requested blob is the app bundle using the authoritative config.
    let app_config = context_client
        .get_context_application(&request.context_id)
        .await;

    if let Ok(app) = app_config {
        let requested_blob = request.blob_id;
        // Allow if it matches the bytecode or compiled artifact
        if requested_blob == app.blob.bytecode || requested_blob == app.blob.compiled {
            debug!(blob_id=%request.blob_id, "Access granted: Blob is public Application Bundle");
            return Ok(true);
        }
    } else {
        warn!("Failed to fetch application config to verify public blob.");
    }

    // Signed-member path. Extracted into a `Store`-and-crypto-only function so
    // the full decision (replay window, signature, direct-or-inherited
    // membership) is unit-testable end to end with a real signature, without an
    // actor or the network.
    is_signed_context_member(context_client.datastore(), request)
}

/// Authorizes a *private* blob read from a signed request: an `auth` envelope
/// must be present, within the replay window, carrying a valid signature from
/// an identity that is a member of the context — **directly** (own
/// ContextIdentity row / direct GroupMember row / namespace-creator admin) OR
/// **by inheritance** through an `Open`-subgroup ancestor.
///
/// Split out of [`is_blob_access_authorized`] (which additionally handles the
/// store-config gate and the public app-bundle allowance, both needing the
/// node client) precisely so this — `Store` + crypto only — can be tested with
/// a real Ed25519 signature and real governance state.
///
/// ## The bug this closes
///
/// The membership gate used to be `ContextClient::has_member` alone, which only
/// sees *direct* membership. A peer who joined an `Open` subgroup via
/// inheritance has NO direct GroupMember row (the apply path
/// `execute_member_joined_open` is validate-only, see list_group_members
/// #2371), so `has_member` returned false and a serving node refused to hand
/// over a blob it owns to a legitimate inherited member. That manifested as
/// one-directional blob (image/canvas) sync: the namespace creator could fetch
/// a joiner's blobs, but the joiner could not fetch the creator's. The
/// inheritance-aware fallback mirrors the sync responder's parent-walk (#2256).
fn is_signed_context_member(
    store: &calimero_store::Store,
    request: &BlobRequest,
) -> eyre::Result<bool> {
    let auth = match &request.auth {
        Some(auth_struct) => auth_struct,
        None => return Ok(false),
    };

    // Replay Protection
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    if auth.timestamp < now.saturating_sub(MAX_REQUEST_AGE_SECS)
        || auth.timestamp > now.saturating_add(MAX_REQUEST_FUTURE_AGE_SECS)
    {
        return Ok(false);
    }

    // Reconstruct the Envelope Payload for Verification
    let payload = BlobAuthPayload {
        blob_id: *request.blob_id,
        context_id: *request.context_id,
        timestamp: auth.timestamp,
    };

    let message = borsh::to_vec(&payload)?;

    // Verify Signature
    if auth
        .public_key
        .verify_raw_signature(&message, &auth.signature)
        .is_err()
    {
        error!(blob_id=%request.blob_id, "The blob request had an auth header, but the signature is incorrect.");
        return Ok(false);
    }

    // Verify Context Membership — direct OR inherited (see the doc comment).
    let mut is_member =
        ContextRegistry::new(store.clone()).has_member(&request.context_id, &auth.public_key)?;
    if !is_member {
        is_member = is_inherited_context_member(store, &request.context_id, &auth.public_key)?;
    }
    if !is_member {
        error!(
            blob_id=%request.blob_id,
            %request.context_id,
            %auth.public_key,
            "The blob request had an auth header, but the identity is not a member of the context."
        );
    }

    Ok(is_member)
}

/// Inheritance-aware context-membership check for blob *read* authorization.
///
/// Resolves the context's owning group and asks the governance store whether
/// `public_key` is a member — directly or by inheritance through an `Open`-
/// subgroup ancestor (the parent-walk implemented by
/// [`MembershipRepository::is_member`] / `check_path`). Returns `false` when
/// the context is not registered to any group (no group binding to inherit
/// through) or when the identity is not a member at any level.
///
/// This intentionally accepts every member *role* (including read-only ones):
/// the predicate gates blob *reads*, which read-only members are entitled to —
/// unlike `is_currently_authorized_for_context`, which gates *writes* and so
/// rejects read-only roles.
fn is_inherited_context_member(
    store: &calimero_store::Store,
    context_id: &calimero_primitives::context::ContextId,
    public_key: &calimero_primitives::identity::PublicKey,
) -> eyre::Result<bool> {
    use calimero_context::group_store::{get_group_for_context, MembershipRepository};

    let Some(group_id) = get_group_for_context(store, context_id)? else {
        return Ok(false);
    };
    MembershipRepository::new(store).is_member(&group_id, public_key)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_context::group_store::{
        register_context_in_group, CapabilitiesRepository, MembershipRepository,
        NamespaceRepository,
    };
    use std::time::{SystemTime, UNIX_EPOCH};

    use calimero_context_config::types::ContextGroupId;
    use calimero_context_config::{MemberCapabilities, VisibilityMode};
    use calimero_network_primitives::blob_types::{BlobAuth, BlobAuthPayload, BlobRequest};
    use calimero_primitives::blobs::BlobId;
    use calimero_primitives::context::{ContextId, GroupMemberRole};
    use calimero_primitives::identity::{PrivateKey, PublicKey};
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;

    use super::{is_inherited_context_member, is_signed_context_member};

    const CONTEXT: [u8; 32] = [0xC0; 32];
    const BLOB: [u8; 32] = [0xD0; 32];

    fn test_store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    /// Build `namespace → Open subgroup → context` where `member` is a direct
    /// member of the *namespace* holding `CAN_JOIN_OPEN_SUBGROUPS` — so they are
    /// an *inherited* member of the subgroup with **no** direct `GroupMember`
    /// row in it — and the context is registered under the subgroup. This is
    /// exactly the shape a peer ends up in after joining an open subgroup.
    fn open_subgroup_with_inherited_member(
        member: &PublicKey,
    ) -> (Store, ContextId, ContextGroupId) {
        let store = test_store();
        let namespace = ContextGroupId::from([0xA0; 32]);
        let subgroup = ContextGroupId::from([0xB0; 32]);
        let context_id = ContextId::from(CONTEXT);

        NamespaceRepository::new(&store)
            .nest(&namespace, &subgroup)
            .unwrap();
        MembershipRepository::new(&store)
            .add_member(&namespace, member, GroupMemberRole::Member)
            .unwrap();
        CapabilitiesRepository::new(&store)
            .set_member_capability(
                &namespace,
                member,
                MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
            )
            .unwrap();
        CapabilitiesRepository::new(&store)
            .set_subgroup_visibility(&subgroup, VisibilityMode::Open)
            .unwrap();
        register_context_in_group(&store, &subgroup, &context_id).unwrap();

        (store, context_id, subgroup)
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    /// A real Ed25519 keypair derived deterministically from a seed byte.
    fn keypair(seed: u8) -> (PrivateKey, PublicKey) {
        let sk = PrivateKey::from([seed; 32]);
        let pk = sk.public_key();
        (sk, pk)
    }

    /// Build a `BlobRequest` for `(BLOB, CONTEXT)` signed by `signer` at
    /// `timestamp`, with the `auth.public_key` set to `claimed` (normally the
    /// signer's own public key; differs only in the signature-mismatch test).
    fn signed_request(signer: &PrivateKey, claimed: PublicKey, timestamp: u64) -> BlobRequest {
        let payload = BlobAuthPayload {
            blob_id: BLOB,
            context_id: CONTEXT,
            timestamp,
        };
        let message = borsh::to_vec(&payload).unwrap();
        let signature = signer.sign(&message).unwrap().to_bytes();
        BlobRequest {
            blob_id: BlobId::from(BLOB),
            context_id: ContextId::from(CONTEXT),
            auth: Some(BlobAuth {
                public_key: claimed,
                signature,
                timestamp,
            }),
        }
    }

    // ── helper-level tests: the inheritance walk ───────────────────────────

    #[test]
    fn inherited_open_subgroup_member_is_recognised() {
        let (_sk, alice) = keypair(0x01);
        let (store, context_id, subgroup) = open_subgroup_with_inherited_member(&alice);

        // Precondition: alice has NO direct membership row in the subgroup —
        // this is precisely why the old flat `has_member` check missed her and
        // blob sync broke one-directionally.
        assert!(
            !MembershipRepository::new(&store)
                .has_direct_member(&subgroup, &alice)
                .unwrap(),
            "test setup invariant: inherited member must have no direct row"
        );
        assert!(
            is_inherited_context_member(&store, &context_id, &alice).unwrap(),
            "inherited Open-subgroup member must be recognised"
        );
    }

    #[test]
    fn restricted_subgroup_does_not_inherit() {
        let (_sk, alice) = keypair(0x01);
        let (store, context_id, subgroup) = open_subgroup_with_inherited_member(&alice);
        CapabilitiesRepository::new(&store)
            .set_subgroup_visibility(&subgroup, VisibilityMode::Restricted)
            .unwrap();
        assert!(
            !is_inherited_context_member(&store, &context_id, &alice).unwrap(),
            "Restricted subgroup must not inherit parent membership"
        );
    }

    #[test]
    fn context_with_no_group_binding_is_not_member() {
        let store = test_store();
        let (_sk, alice) = keypair(0x01);
        assert!(
            !is_inherited_context_member(&store, &ContextId::from([0xC1; 32]), &alice).unwrap(),
            "a context not registered to any group has nothing to inherit through"
        );
    }

    // ── decision-level tests: the full signed-request authorization ────────
    //
    // These exercise the actual function the bug lived in, end to end: replay
    // window + real Ed25519 signature verification + direct-or-inherited
    // membership — against a real governance store, no mocks.

    #[test]
    fn signed_request_from_inherited_member_is_authorized() {
        let (alice_sk, alice_pk) = keypair(0x01);
        let (store, _ctx, _sg) = open_subgroup_with_inherited_member(&alice_pk);

        let request = signed_request(&alice_sk, alice_pk, now_secs());
        assert!(
            is_signed_context_member(&store, &request).unwrap(),
            "an inherited member with a valid signature must be authorized — \
             this is the one-directional-blob-sync regression"
        );
    }

    #[test]
    fn signed_request_from_non_member_is_rejected() {
        let (_alice_sk, alice_pk) = keypair(0x01);
        let (store, _ctx, _sg) = open_subgroup_with_inherited_member(&alice_pk);

        // Mallory signs a perfectly valid request, but was never added to the
        // namespace — membership, not signature validity, must gate access.
        let (mallory_sk, mallory_pk) = keypair(0x99);
        let request = signed_request(&mallory_sk, mallory_pk, now_secs());
        assert!(
            !is_signed_context_member(&store, &request).unwrap(),
            "a validly-signed non-member must be rejected"
        );
    }

    #[test]
    fn signed_request_with_forged_signature_is_rejected() {
        let (_alice_sk, alice_pk) = keypair(0x01);
        let (store, _ctx, _sg) = open_subgroup_with_inherited_member(&alice_pk);

        // Mallory signs but claims to be alice (a real member): the signature
        // won't verify against alice's public key.
        let (mallory_sk, _mallory_pk) = keypair(0x99);
        let request = signed_request(&mallory_sk, alice_pk, now_secs());
        assert!(
            !is_signed_context_member(&store, &request).unwrap(),
            "a signature that doesn't match the claimed public key must be rejected"
        );
    }

    #[test]
    fn signed_request_outside_replay_window_is_rejected() {
        let (alice_sk, alice_pk) = keypair(0x01);
        let (store, _ctx, _sg) = open_subgroup_with_inherited_member(&alice_pk);

        // Valid member, valid signature, but the timestamp is well past the
        // replay window — must be rejected.
        let stale = now_secs() - super::MAX_REQUEST_AGE_SECS - 60;
        let request = signed_request(&alice_sk, alice_pk, stale);
        assert!(
            !is_signed_context_member(&store, &request).unwrap(),
            "a request outside the replay window must be rejected"
        );
    }

    #[test]
    fn request_without_auth_is_rejected() {
        let (_sk, alice) = keypair(0x01);
        let (store, _ctx, _sg) = open_subgroup_with_inherited_member(&alice);

        let request = BlobRequest {
            blob_id: BlobId::from(BLOB),
            context_id: ContextId::from(CONTEXT),
            auth: None,
        };
        assert!(
            !is_signed_context_member(&store, &request).unwrap(),
            "a private blob request without an auth envelope must be rejected"
        );
    }
}
