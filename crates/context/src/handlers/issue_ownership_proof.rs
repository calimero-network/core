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

/// Refuse the request as [`crate::error::ContextError::OwnershipProofInvalid`],
/// a `400` naming the field, where a bare `bail!` would answer the caller's own
/// input as a `500`.
macro_rules! refuse {
    ($($arg:tt)+) => {
        bail!(crate::error::ContextError::OwnershipProofInvalid {
            reason: format!($($arg)+),
        })
    };
}
use calimero_governance_store::MAX_NAMESPACE_DEPTH;
use calimero_governance_store::{MembershipRepository, NamespaceRepository};

/// The claim a proof asserts, named so its three strings cannot be transposed.
///
/// They were three adjacent `&str` parameters. Swapping `audience` and `subject`
/// compiled, and — unlike a swapped epoch, which changes a signature preimage and
/// is refused by the first verifier — it is **silent**: the payload is built from
/// named fields and then signed, so the proof verifies perfectly and simply
/// asserts the wrong thing. A valid signature over a false statement.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ProofClaim<'a> {
    /// Who the proof is addressed to.
    pub(crate) audience: &'a str,
    /// What the proof is about.
    pub(crate) subject: &'a str,
    /// Caller-supplied replay guard.
    pub(crate) nonce: &'a str,
}

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
        refuse!("ownership-proof `{name}` must not be empty");
    }
    if value.len() > MAX_PROOF_FIELD_LEN {
        refuse!(
            "ownership-proof `{name}` is {} bytes; maximum is {MAX_PROOF_FIELD_LEN}",
            value.len()
        );
    }
    if value.chars().any(|c| c.is_control()) {
        refuse!("ownership-proof `{name}` must not contain control characters");
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
///
/// A verifier that cannot read the namespace's governance — mdma — cannot do
/// the admin check, and must not stand `issuer_identity == signer_public_key`
/// in for it: both are the caller's. For a namespace proof the admin API
/// returns, unsigned, the founding pair and this node's credential, from which
/// such a verifier can establish something weaker but checkable: the signer is
/// a node of the account that FOUNDED the namespace (the credential's root
/// certifies `signer_public_key`, and that account with the salt derives
/// `group_id`). An admin the founder promoted later cannot pass that check.
fn validate_proof_fields(audience: &str, subject: &str, nonce: &str) -> eyre::Result<()> {
    validate_proof_field("audience", audience)?;
    validate_proof_field("subject", subject)?;
    validate_proof_field("nonce", nonce)?;
    if nonce.len() < MIN_NONCE_LEN {
        refuse!("ownership-proof `nonce` must be at least {MIN_NONCE_LEN} bytes");
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
    claim: ProofClaim<'_>,
    requested_expires_at_ms: u64,
    now_ms: u64,
) -> eyre::Result<OwnershipProofBuildOutput> {
    let ProofClaim {
        audience,
        subject,
        nonce,
    } = claim;
    validate_proof_fields(audience, subject, nonce)?;

    let node_account = crate::member_account::require(store, &group_id, &node_identity)?;
    if !MembershipRepository::new(store).is_direct_admin(&group_id, &node_account)? {
        bail!(crate::error::ContextError::NotAGroupAdmin {
            group_id: group_id.to_string(),
        });
    }

    let ctx_group = calimero_governance_store::get_group_for_context(store, &context_id)?
        .ok_or_else(|| crate::error::ContextError::ContextNotFound {
            context_id: context_id.to_string(),
        })?;
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
        refuse!("context {context_id:?} is not within the namespace rooted at {group_id:?}");
    }

    // The node's own key, read from where it lives rather than from a per-group
    // copy of it. The identity lookup is gated on participation, so `None` here
    // means this node takes no part in the namespace — which is the real reason
    // it could not sign, and what the old "no signing key registered" message
    // obscured.
    let Some((resolved_pk, signing_key_bytes)) =
        NamespaceRepository::new(store).identity(&group_id)?
    else {
        bail!(crate::error::ContextError::NotANamespaceMember {
            namespace_id: group_id.to_string(),
        });
    };
    if resolved_pk != node_identity {
        bail!(
            "this node signs as {resolved_pk}, not {node_identity}; a node has one signing identity"
        );
    }

    let max_exp = now_ms.saturating_add(MAX_PROOF_LIFETIME_MS);
    let expires_at_ms = requested_expires_at_ms.min(max_exp);
    if expires_at_ms <= now_ms {
        refuse!("expires_at_ms must be in the future");
    }

    // Derive the signer identity from the resolved signing key itself rather
    // than trusting `node_identity` — the check above already proved they
    // agree, but mdma cross-checks that `payload.issuer_identity ==
    // response.signer_public_key` byte-for-byte, so both MUST come from the
    // same source of truth: the private key.
    let private_key = PrivateKey::from(signing_key_bytes);
    let signer_public_key = private_key.public_key();

    let payload = OwnershipClaimPayload {
        v: 1,
        audience,
        group_id: hex::encode(group_id.to_bytes()),
        issuer_identity: signer_public_key.to_string(),
        // Hex, like every other id in this payload. This field was the one
        // base58 holdout, and mdma verifies the payload byte-for-byte, so the
        // change is visible there — as is `issuer_identity` above, which moved
        // to hex with `PublicKey`'s `Display` rather than by an edit here.
        context_id: hex::encode(context_id.as_ref()),
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
    claim: ProofClaim<'_>,
    requested_expires_at_ms: u64,
    now_ms: u64,
) -> eyre::Result<OwnershipProofBuildOutput> {
    let ProofClaim {
        audience,
        subject,
        nonce,
    } = claim;
    validate_proof_fields(audience, subject, nonce)?;

    let node_account = crate::member_account::require(store, &group_id, &node_identity)?;
    if !MembershipRepository::new(store).is_direct_admin(&group_id, &node_account)? {
        bail!(crate::error::ContextError::NotAGroupAdmin {
            group_id: group_id.to_string(),
        });
    }

    // A namespace proof is scoped to a whole namespace, and a namespace IS its
    // root group. Unlike the context path (`build_ownership_proof`), which
    // legitimately accepts a subgroup context via the containment walk, the
    // namespace primitive must reject any non-root `group_id` — admin on a
    // subgroup must not yield a namespace-wide claim. Same check & API as the
    // server-side precedent in
    // `crates/server/src/admin/handlers/namespaces/create_group_in_namespace.rs`.
    if NamespaceRepository::new(store).parent(&group_id)?.is_some() {
        refuse!("group_id must reference a namespace root group");
    }

    // The node's own key, read from where it lives rather than from a per-group
    // copy of it. The identity lookup is gated on participation, so `None` here
    // means this node takes no part in the namespace — which is the real reason
    // it could not sign, and what the old "no signing key registered" message
    // obscured.
    let Some((resolved_pk, signing_key_bytes)) =
        NamespaceRepository::new(store).identity(&group_id)?
    else {
        bail!(crate::error::ContextError::NotANamespaceMember {
            namespace_id: group_id.to_string(),
        });
    };
    if resolved_pk != node_identity {
        bail!(
            "this node signs as {resolved_pk}, not {node_identity}; a node has one signing identity"
        );
    }

    let max_exp = now_ms.saturating_add(MAX_PROOF_LIFETIME_MS);
    let expires_at_ms = requested_expires_at_ms.min(max_exp);
    if expires_at_ms <= now_ms {
        refuse!("expires_at_ms must be in the future");
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
            let (node_identity, _) = self.require_group_signing_key(&req.group_id)?;

            let now_ms = u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())
                .map_err(|_| eyre::eyre!("system clock out of u64 millisecond range"))?;

            let built = build_ownership_proof(
                &self.datastore,
                node_identity,
                req.group_id,
                req.context_id,
                ProofClaim {
                    audience: &req.audience,
                    subject: &req.subject,
                    nonce: &req.nonce,
                },
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
            let (node_identity, _) = self.require_namespace_signing_key(&req.group_id)?;

            let now_ms = u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())
                .map_err(|_| eyre::eyre!("system clock out of u64 millisecond range"))?;

            let built = build_namespace_ownership_proof(
                &self.datastore,
                node_identity,
                req.group_id,
                ProofClaim {
                    audience: &req.audience,
                    subject: &req.subject,
                    nonce: &req.nonce,
                },
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
    /// The refusal reaches the admin API as the named `ContextError` variant,
    /// so it answers its typed status rather than the untyped `500`.
    macro_rules! assert_refused_as {
        ($err:expr, $variant:ident) => {
            assert!(
                matches!(
                    $err.downcast_ref::<crate::error::ContextError>(),
                    Some(crate::error::ContextError::$variant { .. })
                ),
                "expected {}, got: {:#}",
                stringify!($variant),
                $err
            )
        };
    }

    use std::sync::Arc;

    use calimero_context_config::types::ContextGroupId;
    use calimero_primitives::context::{ContextId, GroupMemberRole};
    use calimero_primitives::identity::{PrivateKey, PublicKey};
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;
    use serde_json::Value;

    use super::{
        build_namespace_ownership_proof, build_ownership_proof, ProofClaim, OWNERSHIP_PROOF_DOMAIN,
    };
    use calimero_governance_store;
    use calimero_governance_store::{MembershipRepository, NamespaceRepository};

    const NOW_MS: u64 = 1_700_000_000_000;

    fn test_store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    fn setup_admin_with_node_identity() -> (Store, ContextGroupId, ContextId, PublicKey, PrivateKey)
    {
        let store = test_store();
        let group_id = ContextGroupId::from([0xAA; 32]);
        let context_id = ContextId::from([0xBB; 32]);

        let signing_priv = PrivateKey::from([0x33; 32]);
        let signing_pub = signing_priv.public_key();

        MembershipRepository::new(&store)
            .add_member(
                &group_id,
                &crate::test_support::enrol(&store, &group_id, &signing_pub),
                GroupMemberRole::Admin,
            )
            .expect("add admin");
        NamespaceRepository::new(&store)
            .replace_identity(&group_id, &signing_pub, signing_priv.as_bytes())
            .expect("seed node identity");
        calimero_governance_store::register_context_in_group(&store, &group_id, &context_id)
            .expect("register context");

        (store, group_id, context_id, signing_pub, signing_priv)
    }

    #[test]
    fn happy_path_signature_verifies_and_payload_clamped() {
        let (store, group_id, context_id, signing_pub, _signing_priv) =
            setup_admin_with_node_identity();

        // Request a 1-hour expiry; should be clamped to 5 minutes.
        let requested_expires_at_ms = NOW_MS + (60 * 60 * 1000);
        let out = build_ownership_proof(
            &store,
            signing_pub,
            group_id,
            context_id,
            ProofClaim {
                audience: "mdma.cloud",
                subject: "subject-xyz",
                nonce: "deadbeefcafebabe1122334455667788",
            },
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
        // Bound but never added as a member. The binding matters: an unbound key
        // is refused one step earlier, for not resolving to any principal, and
        // this test is about the direct-admin gate rather than that one.
        let _ = crate::test_support::enrol(&store, &group_id, &identity);

        // Not added as a member at all — not a direct admin.
        let err = build_ownership_proof(
            &store,
            identity,
            group_id,
            context_id,
            ProofClaim {
                audience: "aud",
                subject: "sub",
                nonce: "deadbeefcafebabe1122334455667788",
            },
            NOW_MS + 1_000,
            NOW_MS,
        )
        .expect_err("expected not-direct-admin error");
        assert_refused_as!(err, NotAGroupAdmin);
        assert!(err.to_string().contains("direct admin"));
    }

    #[test]
    fn errors_when_node_takes_no_part_in_the_namespace() {
        let store = test_store();
        let group_id = ContextGroupId::from([0xAA; 32]);
        let context_id = ContextId::from([0xBB; 32]);
        let identity = PublicKey::from([0x44; 32]);

        // Admin row exists and the context is registered, but this node holds
        // no identity for the namespace — it takes no part there.
        MembershipRepository::new(&store)
            .add_member(
                &group_id,
                &crate::test_support::enrol(&store, &group_id, &identity),
                GroupMemberRole::Admin,
            )
            .expect("add admin");
        calimero_governance_store::register_context_in_group(&store, &group_id, &context_id)
            .expect("register context");

        let err = build_ownership_proof(
            &store,
            identity,
            group_id,
            context_id,
            ProofClaim {
                audience: "aud",
                subject: "sub",
                nonce: "deadbeefcafebabe1122334455667788",
            },
            NOW_MS + 1_000,
            NOW_MS,
        )
        .expect_err("expected takes-no-part error");
        assert!(
            matches!(
                err.downcast_ref::<crate::error::ContextError>(),
                Some(crate::error::ContextError::NotANamespaceMember { .. })
            ),
            "a node taking no part in the namespace is a 403 refusal, not a 500; got: {err:#}"
        );
    }

    #[test]
    fn errors_when_expires_at_is_in_the_past() {
        let (store, group_id, context_id, signing_pub, _signing_priv) =
            setup_admin_with_node_identity();

        let err = build_ownership_proof(
            &store,
            signing_pub,
            group_id,
            context_id,
            ProofClaim {
                audience: "aud",
                subject: "sub",
                nonce: "deadbeefcafebabe1122334455667788",
            },
            NOW_MS - 1,
            NOW_MS,
        )
        .expect_err("expected past-expiry error");
        assert_refused_as!(err, OwnershipProofInvalid);
        assert!(err.to_string().contains("expires_at_ms"));
    }

    #[test]
    fn equal_to_now_is_rejected() {
        let (store, group_id, context_id, signing_pub, _signing_priv) =
            setup_admin_with_node_identity();

        let err = build_ownership_proof(
            &store,
            signing_pub,
            group_id,
            context_id,
            ProofClaim {
                audience: "aud",
                subject: "sub",
                nonce: "deadbeefcafebabe1122334455667788",
            },
            NOW_MS,
            NOW_MS,
        )
        .expect_err("expires_at_ms == now must be rejected");
        assert_refused_as!(err, OwnershipProofInvalid);
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

        // Admin + node identity at the root.
        MembershipRepository::new(&store)
            .add_member(
                &root,
                &crate::test_support::enrol(&store, &root, &signing_pub),
                GroupMemberRole::Admin,
            )
            .expect("add admin");
        NamespaceRepository::new(&store)
            .replace_identity(&root, &signing_pub, signing_priv.as_bytes())
            .expect("seed node identity");

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
            ProofClaim {
                audience: "mdma.cloud",
                subject: "subject-xyz",
                nonce: "deadbeefcafebabe1122334455667788",
            },
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
        let (store, group_id, _ctx, signing_pub, _signing_priv) = setup_admin_with_node_identity();

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
            ProofClaim {
                audience: "aud",
                subject: "sub",
                nonce: "deadbeefcafebabe1122334455667788",
            },
            NOW_MS + 1_000,
            NOW_MS,
        )
        .expect_err("context outside the namespace must bail");
        assert_refused_as!(err, OwnershipProofInvalid);
        assert!(err.to_string().contains("not within the namespace"));
    }

    /// A node holds ONE signing identity, so `node_identity` naming anything
    /// other than the key this node actually signs with is a caller error —
    /// and one worth refusing loudly. mdma cross-checks
    /// `payload.issuer_identity == signer_public_key` byte-for-byte, so
    /// silently signing under the resolved key while the caller believed it
    /// was signing under another would produce a proof that fails validation
    /// downstream, with nothing local to point at.
    #[test]
    fn refuses_to_sign_when_node_identity_is_not_this_nodes_key() {
        let store = test_store();
        let group_id = ContextGroupId::from([0xAA; 32]);
        let context_id = ContextId::from([0xBB; 32]);

        // The key this node actually signs with.
        let real_priv = PrivateKey::from([0x33; 32]);
        let real_pub = real_priv.public_key();

        // A different identity the caller asks to sign as.
        let other_identity = PublicKey::from([0x77; 32]);
        assert_ne!(other_identity, real_pub);

        MembershipRepository::new(&store)
            .add_member(
                &group_id,
                &crate::test_support::enrol(&store, &group_id, &other_identity),
                GroupMemberRole::Admin,
            )
            .expect("add admin");
        NamespaceRepository::new(&store)
            .replace_identity(&group_id, &real_pub, real_priv.as_bytes())
            .expect("seed node identity");
        calimero_governance_store::register_context_in_group(&store, &group_id, &context_id)
            .expect("register context");

        let err = build_ownership_proof(
            &store,
            other_identity,
            group_id,
            context_id,
            ProofClaim {
                audience: "mdma.cloud",
                subject: "subject-xyz",
                nonce: "deadbeefcafebabe1122334455667788",
            },
            NOW_MS + 1_000,
            NOW_MS,
        )
        .expect_err("signing as another identity must be refused");
        assert!(
            err.to_string().contains("one signing identity"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn namespace_proof_no_context_succeeds() {
        // Admin + node identity at the namespace-root group, but NO context is
        // registered anywhere. A context-scoped proof would bail on
        // the containment walk; the namespace primitive must succeed.
        let store = test_store();
        let group_id = ContextGroupId::from([0xAA; 32]);

        let signing_priv = PrivateKey::from([0x33; 32]);
        let signing_pub = signing_priv.public_key();

        MembershipRepository::new(&store)
            .add_member(
                &group_id,
                &crate::test_support::enrol(&store, &group_id, &signing_pub),
                GroupMemberRole::Admin,
            )
            .expect("add admin");
        NamespaceRepository::new(&store)
            .replace_identity(&group_id, &signing_pub, signing_priv.as_bytes())
            .expect("seed node identity");

        let out = build_namespace_ownership_proof(
            &store,
            signing_pub,
            group_id,
            ProofClaim {
                audience: "mdma:enable-ha-namespace",
                subject: "subject-xyz",
                nonce: "deadbeefcafebabe1122334455667788",
            },
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
        // Bound, but not a member — see `errors_when_node_is_not_direct_admin`.
        let _ = crate::test_support::enrol(&store, &group_id, &identity);

        // Identity is not a member at all — not a direct admin.
        let err = build_namespace_ownership_proof(
            &store,
            identity,
            group_id,
            ProofClaim {
                audience: "mdma:enable-ha-namespace",
                subject: "sub",
                nonce: "deadbeefcafebabe1122334455667788",
            },
            NOW_MS + 1_000,
            NOW_MS,
        )
        .expect_err("expected not-direct-admin error");
        assert_refused_as!(err, NotAGroupAdmin);
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
        // Enrolled at the ROOT, not the child: bindings live at the namespace
        // anchor and every reader resolves up to it, so a row written against a
        // subgroup goes invisible the moment that subgroup is nested — which is
        // exactly what this test does two statements later.
        MembershipRepository::new(&store)
            .add_member(
                &child,
                &crate::test_support::enrol(&store, &root, &signing_pub),
                GroupMemberRole::Admin,
            )
            .expect("add admin at child");
        NamespaceRepository::new(&store)
            .replace_identity(&root, &signing_pub, signing_priv.as_bytes())
            .expect("seed node identity");

        // `child` is nested under the namespace root `root`, so
        // `NamespaceRepository::new(child).parent()` is `Some(root)`.
        NamespaceRepository::new(&store)
            .nest(&root, &child)
            .expect("nest child under root");

        let err = build_namespace_ownership_proof(
            &store,
            signing_pub,
            child,
            ProofClaim {
                audience: "mdma:enable-ha-namespace",
                subject: "subject-xyz",
                nonce: "deadbeefcafebabe1122334455667788",
            },
            NOW_MS + 1_000,
            NOW_MS,
        )
        .expect_err("namespace proof on a subgroup must bail");
        assert_refused_as!(err, OwnershipProofInvalid);
        assert!(
            err.to_string().contains("root"),
            "error must mention namespace root, got: {err}"
        );
    }

    #[test]
    fn rejects_empty_and_oversized_and_control_and_short_nonce_fields() {
        let (store, group_id, context_id, signing_pub, _sk) = setup_admin_with_node_identity();
        let valid_nonce = "deadbeefcafebabe1122334455667788";
        let over = "x".repeat(super::MAX_PROOF_FIELD_LEN + 1);

        let call = |audience: &str, subject: &str, nonce: &str| {
            build_ownership_proof(
                &store,
                signing_pub,
                group_id,
                context_id,
                ProofClaim {
                    audience,
                    subject,
                    nonce,
                },
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

        // Every one of those is the typed 400, not the untyped 500.
        for (audience, subject, nonce) in [
            ("", "sub", valid_nonce),
            (over.as_str(), "sub", valid_nonce),
            ("aud\n", "sub", valid_nonce),
            ("aud", "sub", "short"),
        ] {
            let err = call(audience, subject, nonce).expect_err("an invalid field");
            assert_refused_as!(err, OwnershipProofInvalid);
        }

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
            ProofClaim {
                audience: "aud",
                subject: "sub",
                nonce: "short",
            },
            NOW_MS + 1_000,
            NOW_MS,
        )
        .expect_err("short nonce must be rejected");
        assert_refused_as!(err, OwnershipProofInvalid);
        assert!(err.to_string().contains("nonce"));
    }

    /// The exact envelope mdma verifies, pinned from this side.
    ///
    /// mdma holds the same vector (`tests/test_ownership_proof_contract_vector.py`)
    /// and runs it through its verifier. The other tests here check that a proof
    /// verifies against *this* crate's key type, which stays true whatever the key
    /// renders as; mdma parses the rendering. Changing the payload's fields, their
    /// order, or an id's spelling breaks HA enable in the cloud, and turns this red.
    #[test]
    fn namespace_proof_matches_the_mdma_contract_vector() {
        const SIGNER_PUBLIC_KEY: &str =
            "17cb79fb2b4120f2b1ec65e4198d6e08b28e813feb01e4a400839b85e18080ce";
        const SIGNED_PAYLOAD: &str = concat!(
            r#"{"v":1,"audience":"mdma:enable-ha-namespace","#,
            r#""group_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","#,
            r#""issuer_identity":"17cb79fb2b4120f2b1ec65e4198d6e08b28e813feb01e4a400839b85e18080ce","#,
            r#""context_id":"","subject":"owner@example.com","#,
            r#""nonce":"deadbeefcafebabe1122334455667788","#,
            r#""issued_at_ms":1700000000000,"expires_at_ms":1700000060000}"#,
        );
        const SIGNATURE: &str = concat!(
            "6424b5b3b22d66c44297bcd7dc02d97b14b6111c54c9a25734c7c5883f076ba9",
            "39ebef31f4652371e36e54f65acff50ac971aa32730486b2019f95b5a83fac00",
        );

        let store = test_store();
        let group_id = ContextGroupId::from([0xAA; 32]);
        let signing_priv = PrivateKey::from([0x33; 32]);
        let signing_pub = signing_priv.public_key();
        MembershipRepository::new(&store)
            .add_member(
                &group_id,
                &crate::test_support::enrol(&store, &group_id, &signing_pub),
                GroupMemberRole::Admin,
            )
            .expect("add admin");
        NamespaceRepository::new(&store)
            .replace_identity(&group_id, &signing_pub, signing_priv.as_bytes())
            .expect("seed node identity");

        let out = build_namespace_ownership_proof(
            &store,
            signing_pub,
            group_id,
            ProofClaim {
                audience: "mdma:enable-ha-namespace",
                subject: "owner@example.com",
                nonce: "deadbeefcafebabe1122334455667788",
            },
            NOW_MS + 60_000,
            NOW_MS,
        )
        .expect("namespace proof");

        // What the admin API reports as `signerPublicKey`.
        assert_eq!(out.signer_public_key.to_string(), SIGNER_PUBLIC_KEY);
        assert_eq!(
            std::str::from_utf8(&out.signed_payload).expect("payload is UTF-8"),
            SIGNED_PAYLOAD
        );
        assert_eq!(hex::encode(out.signature), SIGNATURE);
    }

    /// The full envelope mdma verifies, attachments included, for fixed inputs.
    ///
    /// mdma accepts a namespace proof only when the attached credential's
    /// account root certifies the signing key and that account, with the
    /// attached salt, derives the namespace id. mdma re-implements all three
    /// (borsh `AccountProof<DeviceCert>` layout, the device-cert preimage, the
    /// founding derivation) in Python, so it pins these bytes in
    /// `tests/test_ownership_proof_contract_vector.py`; a drift on either side
    /// turns one of the two red.
    ///
    /// Inputs: account root seed `[0x11; 32]`, node signing seed `[0x33; 32]`,
    /// device id `[0x44; 32]`, agreement key `[0x55; 32]`, salt `[0x66; 32]`,
    /// subject `owner@example.com`, nonce `deadbeefcafebabe1122334455667788`,
    /// issued at `NOW_MS`, expiring 60 s later.
    #[test]
    fn founder_bound_namespace_proof_matches_the_mdma_contract_vector() {
        const ACCOUNT_ID: &str = "c15796f6e49d99e61402c021b59a7f13f40bd86e535716dce10853d31cd7cca4";
        const NAMESPACE_ID: &str =
            "443e4cdc43ec81e977586ff486e79ec6006d089afef1f918da2a6773c0e72008";
        const CREDENTIAL: &str = concat!(
            "02d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c97787",
            "3700000000c15796f6e49d99e61402c021b59a7f13f40bd86e535716dce10853",
            "d31cd7cca4444444444444444444444444444444444444444444444444444444",
            "444444444417cb79fb2b4120f2b1ec65e4198d6e08b28e813feb01e4a400839b",
            "85e18080ce555555555555555555555555555555555555555555555555555555",
            "5555555555000000000000000079134dff69d925fb3a0eafc911cfca9b619795",
            "356823a9d386f6935546618cac243dc46ae2d59faae92be8129d5337543dddd3",
            "b502aa10e52ef6b90b739c0e00",
        );
        const SIGNED_PAYLOAD: &str = concat!(
            r#"{"v":1,"audience":"mdma:enable-ha-namespace","#,
            r#""group_id":"443e4cdc43ec81e977586ff486e79ec6006d089afef1f918da2a6773c0e72008","#,
            r#""issuer_identity":"17cb79fb2b4120f2b1ec65e4198d6e08b28e813feb01e4a400839b85e18080ce","#,
            r#""context_id":"","subject":"owner@example.com","#,
            r#""nonce":"deadbeefcafebabe1122334455667788","#,
            r#""issued_at_ms":1700000000000,"expires_at_ms":1700000060000}"#,
        );
        const SIGNATURE: &str = concat!(
            "c141aaf2605b2a0cf3be0bd51bf61f1d0b4ecab0cfed24c7350aef510b427b6b",
            "20db0a9f84ff5671a0714201f3e722ee32b0d844bedc332ac4d23d2d2104360f",
        );

        let root = PrivateKey::from([0x11; 32]);
        let genesis = calimero_account::AccountGenesis::new(root.public_key());
        let account = genesis.account_id();
        let salt = [0x66; 32];
        let group_id =
            ContextGroupId::from(calimero_account::founded_namespace_id(&account, &salt));

        let signing_priv = PrivateKey::from([0x33; 32]);
        let signing_pub = signing_priv.public_key();
        let credential = calimero_context_client::local_governance::JoinAccountCredential {
            genesis,
            chain: vec![],
            statement: calimero_account::DeviceCert::sign(
                &root,
                account,
                calimero_account::DeviceId::from([0x44; 32]),
                &signing_pub,
                &calimero_account::KemPublicKey::from([0x55; 32]),
                0,
                0,
            )
            .expect("the root certifies the node key"),
        };

        let store = test_store();
        MembershipRepository::new(&store)
            .add_member(
                &group_id,
                &crate::test_support::enrol(&store, &group_id, &signing_pub),
                GroupMemberRole::Admin,
            )
            .expect("add admin");
        NamespaceRepository::new(&store)
            .replace_identity(&group_id, &signing_pub, signing_priv.as_bytes())
            .expect("seed node identity");
        let out = build_namespace_ownership_proof(
            &store,
            signing_pub,
            group_id,
            ProofClaim {
                audience: "mdma:enable-ha-namespace",
                subject: "owner@example.com",
                nonce: "deadbeefcafebabe1122334455667788",
            },
            NOW_MS + 60_000,
            NOW_MS,
        )
        .expect("namespace proof");

        let got = [
            hex::encode(account.as_bytes()),
            hex::encode(group_id.to_bytes()),
            hex::encode(borsh::to_vec(&credential).expect("encode")),
            String::from_utf8(out.signed_payload).expect("payload is UTF-8"),
            hex::encode(out.signature),
        ];
        assert_eq!(
            got,
            [
                ACCOUNT_ID,
                NAMESPACE_ID,
                CREDENTIAL,
                SIGNED_PAYLOAD,
                SIGNATURE
            ],
        );
    }
}
