use std::time::{SystemTime, UNIX_EPOCH};

use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::{IssueOwnershipProofRequest, IssueOwnershipProofResponse};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::Store;
use eyre::bail;
use serde::Serialize;

use crate::group_store;
use crate::ContextManager;

/// Domain-separation tag prepended to the serialized payload before signing.
/// Verifiers MUST reconstruct the signed bytes as `OWNERSHIP_PROOF_DOMAIN ||
/// signed_payload_bytes`.
pub const OWNERSHIP_PROOF_DOMAIN: &[u8] = b"calimero.ownership-claim.v1\x00";

/// Maximum lifetime, in milliseconds, that an issued ownership proof may
/// remain valid. Caller-supplied `expires_at_ms` is clamped to
/// `min(expires_at_ms, issued_at_ms + MAX_PROOF_LIFETIME_MS)`.
pub const MAX_PROOF_LIFETIME_MS: u64 = 5 * 60 * 1000;

/// Canonical ownership-claim payload.
///
/// Field order is locked by the struct definition order (serde_json preserves
/// struct declaration order); changing it changes the byte slice that gets
/// signed and therefore breaks every verifier.
#[derive(Debug, Serialize)]
struct OwnershipClaimPayload<'a> {
    v: u8,
    audience: &'a str,
    group_id: String,
    issuer_identity: String,
    context_id: String,
    subject: &'a str,
    nonce: &'a str,
    issued_at_ms: u64,
    expires_at_ms: u64,
}

/// Result returned by [`build_ownership_proof`], split out so it can be
/// exercised by unit tests without spinning up the actix actor system.
#[derive(Debug)]
pub(crate) struct OwnershipProofBuildOutput {
    pub signer_public_key: PublicKey,
    pub signed_payload: Vec<u8>,
    pub signature: [u8; 64],
}

/// Core handler logic, factored so tests can drive it against an in-memory
/// `Store` and inject a deterministic `now_ms`. The flat argument list is
/// deliberate — every input is part of the locked signed-payload contract
/// (see `OwnershipClaimPayload`), so grouping them into a sub-struct would
/// only obscure the binding between caller fields and signed fields.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_ownership_proof(
    store: &Store,
    node_identity: PublicKey,
    group_id: ContextGroupId,
    context_id: ContextId,
    audience: &str,
    subject: &str,
    nonce: &str,
    requested_expires_at_ms: u64,
    now_ms: u64,
) -> eyre::Result<OwnershipProofBuildOutput> {
    if !group_store::is_direct_group_admin(store, &group_id, &node_identity)? {
        bail!("node is not a direct admin of this group");
    }

    // TODO(#73): verify `context_id` actually belongs to `group_id` (via
    // `enumerate_group_contexts` or a direct lookup). Left as a follow-up
    // because tying it in here changes the test surface significantly and
    // the cross-repo contract only depends on the signed payload bytes.

    let Some(signing_key_bytes) =
        group_store::resolve_group_signing_key(store, &group_id, &node_identity)?
    else {
        bail!("no signing key registered for self-identity in this group");
    };

    let max_exp = now_ms.saturating_add(MAX_PROOF_LIFETIME_MS);
    let expires_at_ms = requested_expires_at_ms.min(max_exp);
    if expires_at_ms <= now_ms {
        bail!("expires_at_ms must be in the future");
    }

    let payload = OwnershipClaimPayload {
        v: 1,
        audience,
        group_id: hex::encode(group_id.to_bytes()),
        issuer_identity: node_identity.to_string(),
        context_id: bs58::encode(context_id.as_ref()).into_string(),
        subject,
        nonce,
        issued_at_ms: now_ms,
        expires_at_ms,
    };
    let signed_payload = serde_json::to_vec(&payload)?;

    let mut sign_input = Vec::with_capacity(OWNERSHIP_PROOF_DOMAIN.len() + signed_payload.len());
    sign_input.extend_from_slice(OWNERSHIP_PROOF_DOMAIN);
    sign_input.extend_from_slice(&signed_payload);

    let private_key = PrivateKey::from(signing_key_bytes);
    let signature = private_key.sign(&sign_input)?;
    let signature_bytes: [u8; 64] = signature.to_bytes();

    Ok(OwnershipProofBuildOutput {
        signer_public_key: node_identity,
        signed_payload,
        signature: signature_bytes,
    })
}

impl Handler<IssueOwnershipProofRequest> for ContextManager {
    type Result = ActorResponse<Self, <IssueOwnershipProofRequest as Message>::Result>;

    fn handle(
        &mut self,
        req: IssueOwnershipProofRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| {
            let Some((node_identity, _)) = self.node_namespace_identity(&req.group_id) else {
                bail!("node has no group identity configured");
            };

            let now_ms = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;

            let built = build_ownership_proof(
                &self.datastore,
                node_identity,
                req.group_id,
                req.context_id,
                &req.audience,
                &req.subject,
                &req.nonce,
                req.expires_at_ms,
                now_ms,
            )?;

            Ok(IssueOwnershipProofResponse {
                signer_public_key: built.signer_public_key,
                signed_payload: built.signed_payload,
                signature: built.signature,
            })
        })();

        ActorResponse::reply(result)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_context_config::types::ContextGroupId;
    use calimero_primitives::context::{ContextId, GroupMemberRole};
    use calimero_primitives::identity::{PrivateKey, PublicKey};
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;
    use serde_json::Value;

    use super::{build_ownership_proof, OWNERSHIP_PROOF_DOMAIN};
    use crate::group_store;

    const NOW_MS: u64 = 1_700_000_000_000;

    fn test_store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    fn setup_admin_with_signing_key() -> (Store, ContextGroupId, ContextId, PublicKey, PrivateKey) {
        let store = test_store();
        let group_id = ContextGroupId::from([0xAA; 32]);
        let context_id = ContextId::from([0xBB; 32]);

        let signing_priv = PrivateKey::from([0x33; 32]);
        let signing_pub = signing_priv.public_key();

        group_store::add_group_member(&store, &group_id, &signing_pub, GroupMemberRole::Admin)
            .expect("add admin");
        group_store::store_group_signing_key(&store, &group_id, &signing_pub, &signing_priv)
            .expect("store signing key");

        (store, group_id, context_id, signing_pub, signing_priv)
    }

    #[test]
    fn happy_path_signature_verifies_and_payload_clamped() {
        let (store, group_id, context_id, signing_pub, _signing_priv) =
            setup_admin_with_signing_key();

        // Request a 1-hour expiry; should be clamped to 5 minutes.
        let requested_expires_at_ms = NOW_MS + (60 * 60 * 1000);
        let out = build_ownership_proof(
            &store,
            signing_pub,
            group_id,
            context_id,
            "mdma.cloud",
            "subject-xyz",
            "deadbeefcafebabe1122334455667788",
            requested_expires_at_ms,
            NOW_MS,
        )
        .expect("happy path");

        // Verify signature: reconstruct sign_input as DOMAIN || signed_payload.
        let mut sign_input =
            Vec::with_capacity(OWNERSHIP_PROOF_DOMAIN.len() + out.signed_payload.len());
        sign_input.extend_from_slice(OWNERSHIP_PROOF_DOMAIN);
        sign_input.extend_from_slice(&out.signed_payload);
        out.signer_public_key
            .verify_raw_signature(&sign_input, &out.signature)
            .expect("signature must verify");

        // Inspect the canonical JSON payload.
        let json: Value =
            serde_json::from_slice(&out.signed_payload).expect("payload must be valid JSON");
        assert_eq!(json["v"], 1);
        assert_eq!(json["audience"], "mdma.cloud");
        assert_eq!(json["subject"], "subject-xyz");
        assert_eq!(json["nonce"], "deadbeefcafebabe1122334455667788");
        assert_eq!(json["issued_at_ms"], NOW_MS);
        // Clamped to NOW_MS + 5*60*1000.
        assert_eq!(json["expires_at_ms"], NOW_MS + 5 * 60 * 1000);
        assert_eq!(json["group_id"], hex::encode([0xAAu8; 32]));
        assert_eq!(json["issuer_identity"], signing_pub.to_string());

        assert_eq!(out.signer_public_key, signing_pub);
    }

    #[test]
    fn errors_when_node_is_not_direct_admin() {
        let store = test_store();
        let group_id = ContextGroupId::from([0xAA; 32]);
        let context_id = ContextId::from([0xBB; 32]);
        let identity = PublicKey::from([0x44; 32]);

        // Not added as a member at all — not a direct admin.
        let err = build_ownership_proof(
            &store,
            identity,
            group_id,
            context_id,
            "aud",
            "sub",
            "deadbeefcafebabe1122334455667788",
            NOW_MS + 1_000,
            NOW_MS,
        )
        .expect_err("expected not-direct-admin error");
        assert!(err.to_string().contains("direct admin"));
    }

    #[test]
    fn errors_when_no_signing_key_registered() {
        let store = test_store();
        let group_id = ContextGroupId::from([0xAA; 32]);
        let context_id = ContextId::from([0xBB; 32]);
        let identity = PublicKey::from([0x44; 32]);

        // Admin row exists but no signing key registered.
        group_store::add_group_member(&store, &group_id, &identity, GroupMemberRole::Admin)
            .expect("add admin");

        let err = build_ownership_proof(
            &store,
            identity,
            group_id,
            context_id,
            "aud",
            "sub",
            "deadbeefcafebabe1122334455667788",
            NOW_MS + 1_000,
            NOW_MS,
        )
        .expect_err("expected missing-signing-key error");
        assert!(err.to_string().contains("signing key"));
    }

    #[test]
    fn errors_when_expires_at_is_in_the_past() {
        let (store, group_id, context_id, signing_pub, _signing_priv) =
            setup_admin_with_signing_key();

        let err = build_ownership_proof(
            &store,
            signing_pub,
            group_id,
            context_id,
            "aud",
            "sub",
            "deadbeefcafebabe1122334455667788",
            NOW_MS - 1,
            NOW_MS,
        )
        .expect_err("expected past-expiry error");
        assert!(err.to_string().contains("expires_at_ms"));
    }

    #[test]
    fn equal_to_now_is_rejected() {
        let (store, group_id, context_id, signing_pub, _signing_priv) =
            setup_admin_with_signing_key();

        let err = build_ownership_proof(
            &store,
            signing_pub,
            group_id,
            context_id,
            "aud",
            "sub",
            "deadbeefcafebabe1122334455667788",
            NOW_MS,
            NOW_MS,
        )
        .expect_err("expires_at_ms == now must be rejected");
        assert!(err.to_string().contains("expires_at_ms"));
    }
}
