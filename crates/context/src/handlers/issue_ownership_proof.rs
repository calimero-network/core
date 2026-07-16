use std::time::{SystemTime, UNIX_EPOCH};

use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::{
    IssueNamespaceOwnershipProofRequest, IssueOwnershipProofRequest, IssueOwnershipProofResponse,
};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::Store;
use eyre::bail;
use serde::Serialize;

use crate::ContextManager;
use calimero_governance_store;
use calimero_governance_store::MAX_NAMESPACE_DEPTH;
use calimero_governance_store::{MembershipRepository, NamespaceRepository, SigningKeysRepository};

/// Domain-separation tag prepended to the serialized payload before signing.
/// Verifiers MUST reconstruct the signed bytes as `OWNERSHIP_PROOF_DOMAIN ||
/// signed_payload_bytes`.
pub const OWNERSHIP_PROOF_DOMAIN: &[u8] = b"calimero.ownership-claim.v1\x00";

/// Maximum lifetime, in milliseconds, that an issued ownership proof may
/// remain valid. Caller-supplied `expires_at_ms` is clamped to
/// `min(expires_at_ms, issued_at_ms + MAX_PROOF_LIFETIME_MS)`.
pub const MAX_PROOF_LIFETIME_MS: u64 = 5 * 60 * 1000;

/// Maximum byte length accepted for each free-form proof field (`audience`,
/// `subject`, `nonce`). These strings are signed verbatim and embedded in the
/// canonical JSON payload, so an unbounded value would let a caller inflate
/// the signed blob without limit. 256 bytes comfortably fits a URL, DID, or
/// public-key string while keeping the signed payload small.
pub const MAX_PROOF_FIELD_LEN: usize = 256;

/// Minimum byte length required of the `nonce`. A one-byte nonce offers no
/// meaningful uniqueness; requiring some width makes accidental collisions far
/// less likely. This is a defence-in-depth floor only — single-use enforcement
/// remains the verifier's responsibility (see the verifier-obligations note
/// below).
pub const MIN_NONCE_LEN: usize = 8;

/// Validate a free-form proof field. Rejects empty, over-long, and values
/// carrying ASCII control characters (which have no legitimate place in an
/// audience/subject/nonce and would render ambiguously across verifiers).
fn validate_proof_field(name: &str, value: &str) -> eyre::Result<()> {
    if value.is_empty() {
        bail!("ownership-proof `{name}` must not be empty");
    }
    if value.len() > MAX_PROOF_FIELD_LEN {
        bail!(
            "ownership-proof `{name}` is {} bytes; maximum is {MAX_PROOF_FIELD_LEN}",
            value.len()
        );
    }
    if value.chars().any(|c| c.is_control()) {
        bail!("ownership-proof `{name}` must not contain control characters");
    }
    Ok(())
}

/// Validate the `audience`/`subject`/`nonce` triple shared by both proof
/// variants.
///
/// # Verifier obligations (MANDATORY — enforced by the relying party, NOT here)
///
/// An issued proof is only an admin's *signed assertion*. Issuance bounds the
/// fields and clamps the lifetime, but it does NOT and CANNOT establish
/// freshness or that the `subject` is entitled to anything. A verifier MUST:
///
///   * reconstruct the signed bytes as `OWNERSHIP_PROOF_DOMAIN || signed_payload`
///     and check the signature against `signer_public_key`;
///   * confirm `payload.issuer_identity == signer_public_key` and that the
///     signer is a current admin of `group_id`;
///   * reject the proof unless `now` is within `[issued_at_ms, expires_at_ms)`;
///   * match `audience` against its own identifier (reject proofs minted for a
///     different relying party);
///   * enforce `nonce` single-use within the proof's validity window (the node
///     keeps no issued-nonce record — replay defence lives entirely here);
///   * treat `subject` as an admin-vouched claim and independently authorize
///     what that subject is allowed to do (an admin can assert any subject).
fn validate_proof_fields(audience: &str, subject: &str, nonce: &str) -> eyre::Result<()> {
    validate_proof_field("audience", audience)?;
    validate_proof_field("subject", subject)?;
    validate_proof_field("nonce", nonce)?;
    if nonce.len() < MIN_NONCE_LEN {
        bail!("ownership-proof `nonce` must be at least {MIN_NONCE_LEN} bytes");
    }
    Ok(())
}

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
    validate_proof_fields(audience, subject, nonce)?;

    if !MembershipRepository::new(store).is_direct_admin(&group_id, &node_identity)? {
        bail!("node is not a direct admin of this group");
    }

    let ctx_group = calimero_governance_store::get_group_for_context(store, &context_id)?
        .ok_or_else(|| eyre::eyre!("context {context_id:?} is not registered in any group"))?;
    // The caller scopes the proof to a namespace root; the context may live in
    // that root or any descendant subgroup. Walk up from the context's group
    // and require we reach `group_id` within the namespace depth bound.
    let mut current = ctx_group;
    let mut contained = current == group_id;
    for _ in 0..MAX_NAMESPACE_DEPTH {
        if contained {
            break;
        }
        match NamespaceRepository::new(store).parent(&current)? {
            Some(parent) => {
                current = parent;
                contained = current == group_id;
            }
            None => break,
        }
    }
    if !contained {
        bail!("context {context_id:?} is not within the namespace rooted at {group_id:?}");
    }

    let Some(signing_key_bytes) =
        SigningKeysRepository::new(store).resolve(&group_id, &node_identity)?
    else {
        bail!("no signing key registered for self-identity in this group");
    };

    let max_exp = now_ms.saturating_add(MAX_PROOF_LIFETIME_MS);
    let expires_at_ms = requested_expires_at_ms.min(max_exp);
    if expires_at_ms <= now_ms {
        bail!("expires_at_ms must be in the future");
    }

    // Derive the signer identity from the resolved signing key itself rather
    // than trusting `node_identity`. In all reachable states these are equal
    // (signing keys are stored keyed by their own public key — see
    // `register_signing_key.rs`), but mdma cross-checks that
    // `payload.issuer_identity == response.signer_public_key` byte-for-byte,
    // so both MUST come from the same source of truth: the private key.
    let private_key = PrivateKey::from(signing_key_bytes);
    let signer_public_key = private_key.public_key();

    let payload = OwnershipClaimPayload {
        v: 1,
        audience,
        group_id: hex::encode(group_id.to_bytes()),
        issuer_identity: signer_public_key.to_string(),
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

    let signature = private_key.sign(&sign_input)?;
    let signature_bytes: [u8; 64] = signature.to_bytes();

    Ok(OwnershipProofBuildOutput {
        signer_public_key,
        signed_payload,
        signature: signature_bytes,
    })
}

/// Namespace-scoped variant of [`build_ownership_proof`].
///
/// This is [`build_ownership_proof`] MINUS the context lookup + containment
/// walk, with `context_id` set to the empty string `""` in the signed
/// payload. The authorization root is unchanged: `is_direct_group_admin` on
/// the namespace-root `group_id`, signing key resolved by `group_id` via
/// `resolve_group_signing_key`, signer derived from the private key, expiry
/// clamp to `now_ms + MAX_PROOF_LIFETIME_MS`. The signed `OwnershipClaimPayload`
/// struct (and therefore its field order / signature input) is reused
/// verbatim; the ONLY delta vs a context proof is `context_id == ""`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_namespace_ownership_proof(
    store: &Store,
    node_identity: PublicKey,
    group_id: ContextGroupId,
    audience: &str,
    subject: &str,
    nonce: &str,
    requested_expires_at_ms: u64,
    now_ms: u64,
) -> eyre::Result<OwnershipProofBuildOutput> {
    validate_proof_fields(audience, subject, nonce)?;

    if !MembershipRepository::new(store).is_direct_admin(&group_id, &node_identity)? {
        bail!("node is not a direct admin of this group");
    }

    // A namespace proof is scoped to a whole namespace, and a namespace IS its
    // root group. Unlike the context path (`build_ownership_proof`), which
    // legitimately accepts a subgroup context via the containment walk, the
    // namespace primitive must reject any non-root `group_id` — admin on a
    // subgroup must not yield a namespace-wide claim. Same check & API as the
    // server-side precedent in
    // `crates/server/src/admin/handlers/namespaces/create_group_in_namespace.rs`.
    if NamespaceRepository::new(store).parent(&group_id)?.is_some() {
        bail!("group_id must reference a namespace root group");
    }

    let Some(signing_key_bytes) =
        SigningKeysRepository::new(store).resolve(&group_id, &node_identity)?
    else {
        bail!("no signing key registered for self-identity in this group");
    };

    let max_exp = now_ms.saturating_add(MAX_PROOF_LIFETIME_MS);
    let expires_at_ms = requested_expires_at_ms.min(max_exp);
    if expires_at_ms <= now_ms {
        bail!("expires_at_ms must be in the future");
    }

    // Derive the signer identity from the resolved signing key itself rather
    // than trusting `node_identity` — identical rationale to
    // `build_ownership_proof`; mdma cross-checks
    // `payload.issuer_identity == response.signer_public_key`.
    let private_key = PrivateKey::from(signing_key_bytes);
    let signer_public_key = private_key.public_key();

    let payload = OwnershipClaimPayload {
        v: 1,
        audience,
        group_id: hex::encode(group_id.to_bytes()),
        issuer_identity: signer_public_key.to_string(),
        // The single, deliberate delta vs a context-scoped proof.
        context_id: String::new(),
        subject,
        nonce,
        issued_at_ms: now_ms,
        expires_at_ms,
    };
    let signed_payload = serde_json::to_vec(&payload)?;

    let mut sign_input = Vec::with_capacity(OWNERSHIP_PROOF_DOMAIN.len() + signed_payload.len());
    sign_input.extend_from_slice(OWNERSHIP_PROOF_DOMAIN);
    sign_input.extend_from_slice(&signed_payload);

    let signature = private_key.sign(&sign_input)?;
    let signature_bytes: [u8; 64] = signature.to_bytes();

    Ok(OwnershipProofBuildOutput {
        signer_public_key,
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

            let now_ms = u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())
                .map_err(|_| eyre::eyre!("system clock out of u64 millisecond range"))?;

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

impl Handler<IssueNamespaceOwnershipProofRequest> for ContextManager {
    type Result = ActorResponse<Self, <IssueNamespaceOwnershipProofRequest as Message>::Result>;

    fn handle(
        &mut self,
        req: IssueNamespaceOwnershipProofRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| {
            let Some((node_identity, _)) = self.node_namespace_identity(&req.group_id) else {
                bail!("node has no group identity configured");
            };

            let now_ms = u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())
                .map_err(|_| eyre::eyre!("system clock out of u64 millisecond range"))?;

            let built = build_namespace_ownership_proof(
                &self.datastore,
                node_identity,
                req.group_id,
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

    use super::{build_namespace_ownership_proof, build_ownership_proof, OWNERSHIP_PROOF_DOMAIN};
    use calimero_governance_store;
    use calimero_governance_store::{
        MembershipRepository, NamespaceRepository, SigningKeysRepository,
    };

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

        MembershipRepository::new(&store)
            .add_member(&group_id, &signing_pub, GroupMemberRole::Admin)
            .expect("add admin");
        SigningKeysRepository::new(&store)
            .store_key(&group_id, &signing_pub, signing_priv.as_bytes())
            .expect("store signing key");
        calimero_governance_store::register_context_in_group(&store, &group_id, &context_id)
            .expect("register context");

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

        // Admin row exists and context is registered, but no signing key.
        MembershipRepository::new(&store)
            .add_member(&group_id, &identity, GroupMemberRole::Admin)
            .expect("add admin");
        calimero_governance_store::register_context_in_group(&store, &group_id, &context_id)
            .expect("register context");

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

    #[test]
    fn context_in_subgroup_of_namespace_root_succeeds() {
        let store = test_store();
        // `root` is the namespace root the caller scopes the proof to;
        // `child` is a descendant subgroup the context actually lives in.
        let root = ContextGroupId::from([0xAA; 32]);
        let child = ContextGroupId::from([0xCC; 32]);
        let context_id = ContextId::from([0xBB; 32]);

        let signing_priv = PrivateKey::from([0x33; 32]);
        let signing_pub = signing_priv.public_key();

        // Admin + signing key registered at the root (resolve_group_signing_key
        // walks up from the requested group, so a root key is reachable).
        MembershipRepository::new(&store)
            .add_member(&root, &signing_pub, GroupMemberRole::Admin)
            .expect("add admin");
        SigningKeysRepository::new(&store)
            .store_key(&root, &signing_pub, signing_priv.as_bytes())
            .expect("store signing key");

        // Context lives in `child`, which is nested under `root`.
        NamespaceRepository::new(&store)
            .nest(&root, &child)
            .expect("nest child under root");
        calimero_governance_store::register_context_in_group(&store, &child, &context_id)
            .expect("register context in subgroup");

        let out = build_ownership_proof(
            &store,
            signing_pub,
            root,
            context_id,
            "mdma.cloud",
            "subject-xyz",
            "deadbeefcafebabe1122334455667788",
            NOW_MS + 1_000,
            NOW_MS,
        )
        .expect("context in subgroup of namespace root must succeed");

        let mut sign_input =
            Vec::with_capacity(OWNERSHIP_PROOF_DOMAIN.len() + out.signed_payload.len());
        sign_input.extend_from_slice(OWNERSHIP_PROOF_DOMAIN);
        sign_input.extend_from_slice(&out.signed_payload);
        out.signer_public_key
            .verify_raw_signature(&sign_input, &out.signature)
            .expect("signature must verify");
    }

    #[test]
    fn context_in_unrelated_group_bails() {
        let (store, group_id, _ctx, signing_pub, _signing_priv) = setup_admin_with_signing_key();

        // A context registered in a group that is neither `group_id` nor a
        // descendant of it must not be claimable under `group_id`.
        let unrelated = ContextGroupId::from([0xEE; 32]);
        let foreign_ctx = ContextId::from([0xDD; 32]);
        calimero_governance_store::register_context_in_group(&store, &unrelated, &foreign_ctx)
            .expect("register context in unrelated group");

        let err = build_ownership_proof(
            &store,
            signing_pub,
            group_id,
            foreign_ctx,
            "aud",
            "sub",
            "deadbeefcafebabe1122334455667788",
            NOW_MS + 1_000,
            NOW_MS,
        )
        .expect_err("context outside the namespace must bail");
        assert!(err.to_string().contains("not within the namespace"));
    }

    #[test]
    fn signer_public_key_is_derived_from_signing_key_not_node_identity() {
        // In all states reachable via `register_signing_key.rs` the stored key
        // is keyed by its own derived public key, so `node_identity` and the
        // signer key coincide. This test deliberately constructs a divergent
        // store state (signing key stored under a `node_identity` that does
        // NOT equal `PrivateKey::from(bytes).public_key()`) to prove the
        // handler derives `signer_public_key` from the key material itself —
        // mdma cross-checks `payload.issuer_identity == signer_public_key`.
        let store = test_store();
        let group_id = ContextGroupId::from([0xAA; 32]);
        let context_id = ContextId::from([0xBB; 32]);

        // The real signing key the proof is produced with.
        let real_priv = PrivateKey::from([0x33; 32]);
        let real_pub = real_priv.public_key();

        // A distinct admin identity used as `node_identity`; the signing key is
        // stored *keyed by this identity* but with `real_priv`'s bytes, so the
        // lookup succeeds yet the derived pubkey differs from node_identity.
        let node_identity = PublicKey::from([0x77; 32]);
        assert_ne!(
            node_identity, real_pub,
            "scenario requires node_identity != derived signing pubkey"
        );

        MembershipRepository::new(&store)
            .add_member(&group_id, &node_identity, GroupMemberRole::Admin)
            .expect("add admin");
        SigningKeysRepository::new(&store)
            .store_key(&group_id, &node_identity, real_priv.as_bytes())
            .expect("store signing key keyed by node_identity");
        calimero_governance_store::register_context_in_group(&store, &group_id, &context_id)
            .expect("register context");

        let out = build_ownership_proof(
            &store,
            node_identity,
            group_id,
            context_id,
            "mdma.cloud",
            "subject-xyz",
            "deadbeefcafebabe1122334455667788",
            NOW_MS + 1_000,
            NOW_MS,
        )
        .expect("happy path with divergent stored key");

        // signer_public_key derives from the signing key, NOT node_identity.
        assert_eq!(out.signer_public_key, real_pub);
        assert_ne!(out.signer_public_key, node_identity);

        // Signature verifies against the derived key.
        let mut sign_input =
            Vec::with_capacity(OWNERSHIP_PROOF_DOMAIN.len() + out.signed_payload.len());
        sign_input.extend_from_slice(OWNERSHIP_PROOF_DOMAIN);
        sign_input.extend_from_slice(&out.signed_payload);
        out.signer_public_key
            .verify_raw_signature(&sign_input, &out.signature)
            .expect("signature must verify against derived signer key");

        // payload.issuer_identity must equal response.signer_public_key.
        let json: Value =
            serde_json::from_slice(&out.signed_payload).expect("payload must be valid JSON");
        assert_eq!(json["issuer_identity"], out.signer_public_key.to_string());
        assert_eq!(json["issuer_identity"], real_pub.to_string());
    }

    #[test]
    fn namespace_proof_no_context_succeeds() {
        // Admin + signing key registered at the namespace-root group, but NO
        // context is registered anywhere. A context-scoped proof would bail on
        // the containment walk; the namespace primitive must succeed.
        let store = test_store();
        let group_id = ContextGroupId::from([0xAA; 32]);

        let signing_priv = PrivateKey::from([0x33; 32]);
        let signing_pub = signing_priv.public_key();

        MembershipRepository::new(&store)
            .add_member(&group_id, &signing_pub, GroupMemberRole::Admin)
            .expect("add admin");
        SigningKeysRepository::new(&store)
            .store_key(&group_id, &signing_pub, signing_priv.as_bytes())
            .expect("store signing key");

        let out = build_namespace_ownership_proof(
            &store,
            signing_pub,
            group_id,
            "mdma:enable-ha-namespace",
            "subject-xyz",
            "deadbeefcafebabe1122334455667788",
            NOW_MS + 1_000,
            NOW_MS,
        )
        .expect("namespace proof with no context must succeed");

        // Signature verifies over DOMAIN || signed_payload.
        let mut sign_input =
            Vec::with_capacity(OWNERSHIP_PROOF_DOMAIN.len() + out.signed_payload.len());
        sign_input.extend_from_slice(OWNERSHIP_PROOF_DOMAIN);
        sign_input.extend_from_slice(&out.signed_payload);
        out.signer_public_key
            .verify_raw_signature(&sign_input, &out.signature)
            .expect("signature must verify");

        let json: Value =
            serde_json::from_slice(&out.signed_payload).expect("payload must be valid JSON");
        assert_eq!(json["v"], 1);
        // The only delta vs a context proof: context_id is the empty string.
        assert_eq!(json["context_id"], "");
        // Audience is passed through unchanged; core hardcodes nothing.
        assert_eq!(json["audience"], "mdma:enable-ha-namespace");
        assert_eq!(json["subject"], "subject-xyz");
        assert_eq!(json["group_id"], hex::encode([0xAAu8; 32]));
        assert_eq!(json["issuer_identity"], signing_pub.to_string());
        assert_eq!(out.signer_public_key, signing_pub);
    }

    #[test]
    fn non_admin_namespace_proof_bails() {
        let store = test_store();
        let group_id = ContextGroupId::from([0xAA; 32]);
        let identity = PublicKey::from([0x44; 32]);

        // Identity is not a member at all — not a direct admin.
        let err = build_namespace_ownership_proof(
            &store,
            identity,
            group_id,
            "mdma:enable-ha-namespace",
            "sub",
            "deadbeefcafebabe1122334455667788",
            NOW_MS + 1_000,
            NOW_MS,
        )
        .expect_err("expected not-direct-admin error");
        assert!(err.to_string().contains("direct admin"));
    }

    #[test]
    fn subgroup_namespace_proof_bails() {
        // A namespace proof must be scoped to a namespace ROOT (a group with
        // no parent). Being a direct admin of a subgroup nested under a root
        // must NOT yield a namespace-wide claim: the builder must bail.
        let store = test_store();
        let root = ContextGroupId::from([0xAA; 32]);
        let child = ContextGroupId::from([0xCC; 32]);

        let signing_priv = PrivateKey::from([0x33; 32]);
        let signing_pub = signing_priv.public_key();

        // Admin + signing key registered directly at the child subgroup, so
        // the `is_direct_group_admin` gate passes for `child` — the only
        // thing standing between the caller and a namespace proof is the
        // namespace-root check.
        MembershipRepository::new(&store)
            .add_member(&child, &signing_pub, GroupMemberRole::Admin)
            .expect("add admin at child");
        SigningKeysRepository::new(&store)
            .store_key(&child, &signing_pub, signing_priv.as_bytes())
            .expect("store signing key at child");

        // `child` is nested under the namespace root `root`, so
        // `NamespaceRepository::new(child).parent()` is `Some(root)`.
        NamespaceRepository::new(&store)
            .nest(&root, &child)
            .expect("nest child under root");

        let err = build_namespace_ownership_proof(
            &store,
            signing_pub,
            child,
            "mdma:enable-ha-namespace",
            "subject-xyz",
            "deadbeefcafebabe1122334455667788",
            NOW_MS + 1_000,
            NOW_MS,
        )
        .expect_err("namespace proof on a subgroup must bail");
        assert!(
            err.to_string().contains("root"),
            "error must mention namespace root, got: {err}"
        );
    }

    #[test]
    fn rejects_empty_and_oversized_and_control_and_short_nonce_fields() {
        let (store, group_id, context_id, signing_pub, _sk) = setup_admin_with_signing_key();
        let valid_nonce = "deadbeefcafebabe1122334455667788";
        let over = "x".repeat(super::MAX_PROOF_FIELD_LEN + 1);

        let call = |audience: &str, subject: &str, nonce: &str| {
            build_ownership_proof(
                &store,
                signing_pub,
                group_id,
                context_id,
                audience,
                subject,
                nonce,
                NOW_MS + 1_000,
                NOW_MS,
            )
        };

        // Empty fields.
        assert!(call("", "sub", valid_nonce)
            .expect_err("empty audience")
            .to_string()
            .contains("audience"));
        assert!(call("aud", "", valid_nonce)
            .expect_err("empty subject")
            .to_string()
            .contains("subject"));
        assert!(call("aud", "sub", "")
            .expect_err("empty nonce")
            .to_string()
            .contains("nonce"));

        // Oversized field.
        assert!(call(&over, "sub", valid_nonce)
            .expect_err("oversized audience")
            .to_string()
            .contains("maximum"));

        // Control characters.
        assert!(call("aud\n", "sub", valid_nonce)
            .expect_err("control char in audience")
            .to_string()
            .contains("control"));

        // Nonce below the minimum width.
        assert!(call("aud", "sub", "short")
            .expect_err("short nonce")
            .to_string()
            .contains("nonce"));

        // The exact-max boundary and a valid nonce are accepted (fields are
        // validated before the admin/signing-key checks, so this reaches the
        // happy path).
        let at_max = "x".repeat(super::MAX_PROOF_FIELD_LEN);
        call(&at_max, "sub", valid_nonce).expect("field at max length is accepted");
    }

    #[test]
    fn namespace_proof_validates_fields_too() {
        let store = test_store();
        let group_id = ContextGroupId::from([0xAA; 32]);
        let identity = PublicKey::from([0x44; 32]);

        // A bad field is rejected before the admin check, so we don't need a
        // fully-set-up admin to observe the field validation firing.
        let err = build_namespace_ownership_proof(
            &store,
            identity,
            group_id,
            "aud",
            "sub",
            "short",
            NOW_MS + 1_000,
            NOW_MS,
        )
        .expect_err("short nonce must be rejected");
        assert!(err.to_string().contains("nonce"));
    }
}
