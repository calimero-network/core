//! Signed group operations for **local** governance (no chain).
//!
//! See `docs/context-management/LOCAL-GROUP-GOVERNANCE.md`.
//!
//! ## Namespace governance model
//!
//! A **namespace** has a single governance DAG. Operations in the DAG are
//! either *root ops* (cleartext, visible to all namespace members) or
//! *group-scoped ops* (tagged with a cleartext `group_id` for routing, but
//! the payload is encrypted with the group's sender key so only group
//! members can read it). Non-members store an opaque **skeleton** for
//! group-scoped ops they cannot decrypt.

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_context_config::types::SignedGroupOpenInvitation;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, GroupMemberRole, UpgradePolicy};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use ed25519_dalek::SignatureError;
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Wire/schema version for [`SignedGroupOp`].
///
/// v3: Added `state_hash: [u8; 32]` — each op commits to the group's
/// authorization-relevant state at signing time. On apply, a non-zero
/// state hash is verified against the current state to reject stale ops.
///
/// v2: `parent_op_hash: Option` changed to `parent_op_hashes: Vec` for
/// multi-parent DAG support. See `DAG-BASED-GOVERNANCE.md`.
///
/// v1 was internal to feature branch development and never deployed to any
/// persistent network. No backward-compatible deserialization is needed.
pub const SIGNED_GROUP_OP_SCHEMA_VERSION: u8 = 3;

/// Domain separation prefix for Ed25519 signatures over group ops.
pub const GROUP_GOVERNANCE_SIGN_DOMAIN: &[u8] = b"calimero.group.v1";

/// Group mutation for local governance (signed, gossip-replicated).
///
/// Aligns with CLI / contract surfaces where feasible; see
/// `docs/context-management/LOCAL-GROUP-GOVERNANCE.md`.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
#[non_exhaustive]
pub enum GroupOp {
    /// Reserved for tests / padding.
    Noop,
    /// Add a member with a role.
    MemberAdded {
        member: PublicKey,
        role: GroupMemberRole,
    },
    /// Remove a member.
    MemberRemoved { member: PublicKey },
    /// Set a member’s role (same as upsert member with new role).
    MemberRoleSet {
        member: PublicKey,
        role: GroupMemberRole,
    },
    /// Per-member capability bitmask (`GroupMemberCapability` store).
    MemberCapabilitySet {
        member: PublicKey,
        capabilities: u32,
    },
    /// Default capability bitmask for new members.
    DefaultCapabilitiesSet { capabilities: u32 },
    /// Update group upgrade policy in [`GroupMetaValue`].
    UpgradePolicySet { policy: UpgradePolicy },
    /// Update target application and app key in group metadata.
    TargetApplicationSet {
        app_key: [u8; 32],
        target_application_id: ApplicationId,
    },
    /// Register a context index under this group (must match `ContextGroupRef` invariants).
    ContextRegistered { context_id: ContextId },
    /// Unregister a context from this group.
    ContextDetached { context_id: ContextId },
    /// Default visibility for new contexts (`0` = Open, `1` = Restricted).
    DefaultVisibilitySet { mode: u8 },
    /// Human-readable alias for a context within the group.
    /// **Signer:** group admin.
    ContextAliasSet {
        context_id: ContextId,
        alias: String,
    },
    /// Human-readable alias for a member within the group.
    MemberAliasSet { member: PublicKey, alias: String },
    /// Human-readable alias for the group itself.
    GroupAliasSet { alias: String },
    /// Delete the group locally (no registered contexts; same constraints as CLI delete).
    GroupDelete,
    /// Update group migration bytes in [`GroupMetaValue`] (admin).
    GroupMigrationSet { migration: Option<Vec<u8>> },
    /// Grant a capability to a member for a specific context.
    ContextCapabilityGranted {
        context_id: ContextId,
        member: PublicKey,
        capability: u8,
    },
    /// Revoke a capability from a member for a specific context.
    ContextCapabilityRevoked {
        context_id: ContextId,
        member: PublicKey,
        capability: u8,
    },
    /// TEE admission policy: defines which TEE nodes can auto-join the group.
    /// Only admins can set this policy.
    TeeAdmissionPolicySet {
        allowed_mrtd: Vec<String>,
        allowed_rtmr0: Vec<String>,
        allowed_rtmr1: Vec<String>,
        allowed_rtmr2: Vec<String>,
        allowed_rtmr3: Vec<String>,
        allowed_tcb_statuses: Vec<String>,
        accept_mock: bool,
    },
    /// A TEE node was admitted via attestation that matched the group's policy.
    /// Signed by an existing member who verified the attestation.
    MemberJoinedViaTeeAttestation {
        member: PublicKey,
        quote_hash: [u8; 32],
        mrtd: String,
        rtmr0: String,
        rtmr1: String,
        rtmr2: String,
        rtmr3: String,
        tcb_status: String,
        role: GroupMemberRole,
    },
}

// ---------------------------------------------------------------------------
// Namespace-scoped governance ops (Phase 2 rewrite)
// ---------------------------------------------------------------------------

/// Top-level operation in the single namespace governance DAG.
///
/// Every delta in the DAG carries exactly one `NamespaceOp`:
/// - `Root` ops are cleartext and visible to all namespace members.
/// - `Group` ops have a cleartext `group_id` tag (for topic routing and
///    skeleton storage) but the actual mutation is encrypted.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub enum NamespaceOp {
    /// Cleartext namespace-wide administrative operation.
    Root(RootOp),
    /// Encrypted group-scoped operation. The `group_id` is cleartext so
    /// non-members can store the skeleton; the payload is only readable
    /// by holders of the group's sender key.
    Group {
        group_id: [u8; 32],
        encrypted: EncryptedGroupOp,
    },
}

/// Cleartext administrative operations that affect the entire namespace.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub enum RootOp {
    /// A new group was created within this namespace.
    GroupCreated { group_id: [u8; 32] },
    /// A group was deleted from this namespace.
    GroupDeleted { group_id: [u8; 32] },
    /// The namespace administrator was changed.
    AdminChanged { new_admin: PublicKey },
    /// Namespace-wide policy was updated (extensible).
    PolicyUpdated { policy_bytes: Vec<u8> },
    /// Record that `child_group_id` is nested inside `parent_group_id`.
    /// Purely organizational metadata — does NOT grant permission inheritance.
    /// Both groups must already exist in this namespace.
    GroupNested {
        parent_group_id: [u8; 32],
        child_group_id: [u8; 32],
    },
    /// Remove a nesting relationship.
    GroupUnnested {
        parent_group_id: [u8; 32],
        child_group_id: [u8; 32],
    },
    /// A member joined a group via an admin-signed invitation.
    ///
    /// **Cleartext** because the joiner doesn't hold the group's
    /// sender_key yet. The outer `SignedNamespaceOp` MUST be signed by
    /// the joining member (proves key ownership). Peers verify:
    ///
    /// 1. `signed_invitation.inviter_signature` is from a group admin
    /// 2. `signed_invitation.invitation.group_id` matches this op's context
    /// 3. `SignedNamespaceOp.signer` == `member` (can't add someone else)
    /// 4. The invitation hasn't expired
    ///
    /// The **role** is inside `signed_invitation.invitation.invited_role`
    /// (covered by admin's signature, joiner cannot escalate).
    ///
    /// After peers apply this, they deliver the group `sender_key` to
    /// the new member via the key-share protocol.
    MemberJoined {
        member: PublicKey,
        /// The full admin-signed invitation — carries the inviter's
        /// identity, group_id, expiration, role, and the admin's
        /// signature. Peers use this to verify the join was authorized.
        signed_invitation: SignedGroupOpenInvitation,
    },
}

/// An encrypted group operation payload. Only members of the group
/// (who possess the sender key) can decrypt the inner [`GroupOp`].
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct EncryptedGroupOp {
    /// 12-byte AES-GCM nonce.
    pub nonce: [u8; 12],
    /// `AES-256-GCM(borsh(GroupOp))` using the group's sender key.
    pub ciphertext: Vec<u8>,
}

/// Signable envelope for a namespace governance operation.
///
/// Mirrors [`SignableGroupOp`] but wraps [`NamespaceOp`] instead of
/// [`GroupOp`], and is scoped to a namespace rather than a single group.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct SignableNamespaceOp {
    pub version: u8,
    pub namespace_id: [u8; 32],
    pub parent_op_hashes: Vec<[u8; 32]>,
    pub state_hash: [u8; 32],
    pub signer: PublicKey,
    pub nonce: u64,
    pub op: NamespaceOp,
}

/// A signed namespace governance operation ready for gossip or storage.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct SignedNamespaceOp {
    pub version: u8,
    pub namespace_id: [u8; 32],
    pub parent_op_hashes: Vec<[u8; 32]>,
    pub state_hash: [u8; 32],
    pub signer: PublicKey,
    pub nonce: u64,
    pub op: NamespaceOp,
    pub signature: [u8; 64],
}

/// Wire/schema version for [`SignedNamespaceOp`].
pub const SIGNED_NAMESPACE_OP_SCHEMA_VERSION: u8 = 1;

/// Domain separation prefix for Ed25519 signatures over namespace ops.
pub const NAMESPACE_GOVERNANCE_SIGN_DOMAIN: &[u8] = b"calimero.namespace.v1";

/// Bytes that are hashed/signed for a namespace op.
pub fn namespace_signable_bytes(
    signable: &SignableNamespaceOp,
) -> Result<Vec<u8>, GovernanceError> {
    let mut body =
        borsh::to_vec(signable).map_err(|e| GovernanceError::BorshSerialize(e.to_string()))?;
    let mut out = Vec::with_capacity(NAMESPACE_GOVERNANCE_SIGN_DOMAIN.len() + body.len());
    out.extend_from_slice(NAMESPACE_GOVERNANCE_SIGN_DOMAIN);
    out.append(&mut body);
    Ok(out)
}

/// Content hash for a namespace op (SHA-256 of [`namespace_signable_bytes`]).
#[must_use]
pub fn namespace_op_content_hash(
    signable: &SignableNamespaceOp,
) -> Result<[u8; 32], GovernanceError> {
    let bytes = namespace_signable_bytes(signable)?;
    Ok(Sha256::digest(&bytes).into())
}

impl SignedNamespaceOp {
    /// Build and sign a new namespace operation.
    pub fn sign(
        sk: &PrivateKey,
        namespace_id: [u8; 32],
        parent_op_hashes: Vec<[u8; 32]>,
        state_hash: [u8; 32],
        nonce: u64,
        op: NamespaceOp,
    ) -> Result<Self, GovernanceError> {
        let signer = sk.public_key();
        let signable = SignableNamespaceOp {
            version: SIGNED_NAMESPACE_OP_SCHEMA_VERSION,
            namespace_id,
            parent_op_hashes,
            state_hash,
            signer,
            nonce,
            op,
        };
        let msg = namespace_signable_bytes(&signable)?;
        let sig = sk.sign(&msg)?;
        Ok(Self {
            version: signable.version,
            namespace_id: signable.namespace_id,
            parent_op_hashes: signable.parent_op_hashes,
            state_hash: signable.state_hash,
            signer: signable.signer,
            nonce: signable.nonce,
            op: signable.op,
            signature: sig.to_bytes(),
        })
    }

    /// Verify schema version and Ed25519 signature.
    pub fn verify_signature(&self) -> Result<(), GovernanceError> {
        if self.version != SIGNED_NAMESPACE_OP_SCHEMA_VERSION {
            return Err(GovernanceError::SchemaVersion {
                expected: SIGNED_NAMESPACE_OP_SCHEMA_VERSION,
                got: self.version,
            });
        }
        let signable = self.to_signable();
        let msg = namespace_signable_bytes(&signable)?;
        self.signer.verify_raw_signature(&msg, &self.signature)?;
        Ok(())
    }

    #[must_use]
    pub fn to_signable(&self) -> SignableNamespaceOp {
        SignableNamespaceOp {
            version: self.version,
            namespace_id: self.namespace_id,
            parent_op_hashes: self.parent_op_hashes.clone(),
            state_hash: self.state_hash,
            signer: self.signer,
            nonce: self.nonce,
            op: self.op.clone(),
        }
    }

    /// Content hash for this op's signable payload (for dedup / parent links).
    pub fn content_hash(&self) -> Result<[u8; 32], GovernanceError> {
        namespace_op_content_hash(&self.to_signable())
    }

    /// Extract the group_id if this is a group-scoped op.
    #[must_use]
    pub fn group_id(&self) -> Option<[u8; 32]> {
        match &self.op {
            NamespaceOp::Group { group_id, .. } => Some(*group_id),
            NamespaceOp::Root(_) => None,
        }
    }
}

/// An opaque skeleton stored by non-members for group-scoped ops they
/// cannot decrypt. Retains causal structure so the DAG remains valid.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct OpaqueSkeleton {
    pub delta_id: [u8; 32],
    pub parent_op_hashes: Vec<[u8; 32]>,
    pub group_id: [u8; 32],
    pub signer: PublicKey,
}

// ---------------------------------------------------------------------------
// Original group-scoped types (kept for backward compat and as inner payload)
// ---------------------------------------------------------------------------

/// Payload that is actually signed (everything except the signature bytes).
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct SignableGroupOp {
    pub version: u8,
    pub group_id: [u8; 32],
    pub parent_op_hashes: Vec<[u8; 32]>,
    pub state_hash: [u8; 32],
    pub signer: PublicKey,
    pub nonce: u64,
    pub op: GroupOp,
}

/// A signed group operation ready for gossip or storage.
///
/// Embeds DAG parent references for causal ordering: `parent_op_hashes`
/// contains the content hashes of all current DAG heads at signing time.
/// Single parent = linear chain; multiple parents = merge after concurrent ops.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct SignedGroupOp {
    pub version: u8,
    pub group_id: [u8; 32],
    pub parent_op_hashes: Vec<[u8; 32]>,
    pub state_hash: [u8; 32],
    pub signer: PublicKey,
    pub nonce: u64,
    pub op: GroupOp,
    pub signature: [u8; 64],
}

#[derive(Debug, Error)]
pub enum GovernanceError {
    #[error("schema version mismatch: expected {expected}, got {got}")]
    SchemaVersion { expected: u8, got: u8 },
    #[error("signature verification failed: {0}")]
    Signature(#[from] SignatureError),
    #[error("borsh serialization failed: {0}")]
    BorshSerialize(String),
}

/// Bytes that are hashed/signed: `GROUP_GOVERNANCE_SIGN_DOMAIN` || `borsh(SignableGroupOp)`.
pub fn signable_bytes(signable: &SignableGroupOp) -> Result<Vec<u8>, GovernanceError> {
    let mut body =
        borsh::to_vec(signable).map_err(|e| GovernanceError::BorshSerialize(e.to_string()))?;
    let mut out = Vec::with_capacity(GROUP_GOVERNANCE_SIGN_DOMAIN.len() + body.len());
    out.extend_from_slice(GROUP_GOVERNANCE_SIGN_DOMAIN);
    out.append(&mut body);
    Ok(out)
}

/// Stable content id for idempotency: SHA-256 of [`signable_bytes`].
#[must_use]
pub fn op_content_hash(signable: &SignableGroupOp) -> Result<[u8; 32], GovernanceError> {
    let bytes = signable_bytes(signable)?;
    Ok(Sha256::digest(&bytes).into())
}

impl SignedGroupOp {
    /// Build and sign a new operation with [`SIGNED_GROUP_OP_SCHEMA_VERSION`].
    ///
    /// `parent_op_hashes` should be the current DAG heads (content hashes of the
    /// latest applied ops). Empty vec for the first op in a group (genesis).
    pub fn sign(
        sk: &PrivateKey,
        group_id: [u8; 32],
        parent_op_hashes: Vec<[u8; 32]>,
        state_hash: [u8; 32],
        nonce: u64,
        op: GroupOp,
    ) -> Result<Self, GovernanceError> {
        let signer = sk.public_key();
        let signable = SignableGroupOp {
            version: SIGNED_GROUP_OP_SCHEMA_VERSION,
            group_id,
            parent_op_hashes,
            state_hash,
            signer,
            nonce,
            op,
        };
        let msg = signable_bytes(&signable)?;
        let sig = sk.sign(&msg)?;
        Ok(Self {
            version: signable.version,
            group_id: signable.group_id,
            parent_op_hashes: signable.parent_op_hashes,
            state_hash: signable.state_hash,
            signer: signable.signer,
            nonce: signable.nonce,
            op: signable.op,
            signature: sig.to_bytes(),
        })
    }

    /// Verify schema version and Ed25519 signature.
    pub fn verify_signature(&self) -> Result<(), GovernanceError> {
        if self.version != SIGNED_GROUP_OP_SCHEMA_VERSION {
            return Err(GovernanceError::SchemaVersion {
                expected: SIGNED_GROUP_OP_SCHEMA_VERSION,
                got: self.version,
            });
        }
        let signable = self.to_signable();
        let msg = signable_bytes(&signable)?;
        self.signer.verify_raw_signature(&msg, &self.signature)?;
        Ok(())
    }

    #[must_use]
    pub fn to_signable(&self) -> SignableGroupOp {
        SignableGroupOp {
            version: self.version,
            group_id: self.group_id,
            parent_op_hashes: self.parent_op_hashes.clone(),
            state_hash: self.state_hash,
            signer: self.signer,
            nonce: self.nonce,
            op: self.op.clone(),
        }
    }

    /// Content hash for this op’s signable payload (for dedup / parent links).
    pub fn content_hash(&self) -> Result<[u8; 32], GovernanceError> {
        op_content_hash(&self.to_signable())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    fn sample_group_id() -> [u8; 32] {
        let mut g = [0u8; 32];
        g[0] = 7;
        g[31] = 3;
        g
    }

    #[test]
    fn sign_and_verify_round_trip() {
        let mut rng = OsRng;
        let sk = PrivateKey::random(&mut rng);
        let member = PrivateKey::random(&mut rng).public_key();

        let op = SignedGroupOp::sign(
            &sk,
            sample_group_id(),
            vec![],
            [0u8; 32],
            1,
            GroupOp::MemberAdded {
                member,
                role: GroupMemberRole::Member,
            },
        )
        .expect("sign");

        op.verify_signature().expect("verify");
    }

    #[test]
    fn wrong_key_fails() {
        let mut rng = OsRng;
        let sk = PrivateKey::random(&mut rng);
        let other = PrivateKey::random(&mut rng);
        let member = PrivateKey::random(&mut rng).public_key();

        let mut op = SignedGroupOp::sign(
            &sk,
            sample_group_id(),
            vec![],
            [0u8; 32],
            1,
            GroupOp::MemberAdded {
                member,
                role: GroupMemberRole::Admin,
            },
        )
        .expect("sign");

        // Swap signer to another key without re-signing
        op.signer = other.public_key();

        assert!(op.verify_signature().is_err());
    }

    #[test]
    fn tampered_op_fails() {
        let mut rng = OsRng;
        let sk = PrivateKey::random(&mut rng);
        let member = PrivateKey::random(&mut rng).public_key();

        let mut op = SignedGroupOp::sign(
            &sk,
            sample_group_id(),
            vec![],
            [0u8; 32],
            1,
            GroupOp::MemberAdded {
                member,
                role: GroupMemberRole::Member,
            },
        )
        .expect("sign");

        op.nonce = 2;
        assert!(op.verify_signature().is_err());
    }

    #[test]
    fn replay_distinct_content_hash() {
        let mut rng = OsRng;
        let sk = PrivateKey::random(&mut rng);
        let member = PrivateKey::random(&mut rng).public_key();

        let op1 = SignedGroupOp::sign(
            &sk,
            sample_group_id(),
            vec![],
            [0u8; 32],
            1,
            GroupOp::MemberAdded {
                member,
                role: GroupMemberRole::Member,
            },
        )
        .expect("sign");

        let op2 = SignedGroupOp::sign(
            &sk,
            sample_group_id(),
            vec![],
            [0u8; 32],
            2,
            GroupOp::MemberAdded {
                member,
                role: GroupMemberRole::Member,
            },
        )
        .expect("sign");

        let h1 = op1.content_hash().expect("hash");
        let h2 = op2.content_hash().expect("hash");
        assert_ne!(
            h1, h2,
            "different nonces must yield different content hashes"
        );
    }

    #[test]
    fn signable_bytes_deterministic() {
        let mut rng = OsRng;
        let sk = PrivateKey::random(&mut rng);
        let pk = sk.public_key();
        let s = SignableGroupOp {
            version: SIGNED_GROUP_OP_SCHEMA_VERSION,
            group_id: [1u8; 32],
            parent_op_hashes: vec![],
            state_hash: [0u8; 32],
            signer: pk,
            nonce: 42,
            op: GroupOp::Noop,
        };
        let a = signable_bytes(&s).expect("bytes");
        let b = signable_bytes(&s).expect("bytes");
        assert_eq!(a, b);
        assert!(a.starts_with(GROUP_GOVERNANCE_SIGN_DOMAIN));
    }

    // --- Namespace op tests ---

    fn sample_namespace_id() -> [u8; 32] {
        let mut ns = [0u8; 32];
        ns[0] = 0xAA;
        ns[31] = 0xBB;
        ns
    }

    #[test]
    fn namespace_op_sign_verify_root() {
        let mut rng = OsRng;
        let sk = PrivateKey::random(&mut rng);

        let op = SignedNamespaceOp::sign(
            &sk,
            sample_namespace_id(),
            vec![],
            [0u8; 32],
            1,
            NamespaceOp::Root(RootOp::GroupCreated {
                group_id: sample_group_id(),
            }),
        )
        .expect("sign");

        op.verify_signature().expect("verify");
        assert!(op.group_id().is_none());
    }

    #[test]
    fn namespace_op_sign_verify_group() {
        let mut rng = OsRng;
        let sk = PrivateKey::random(&mut rng);

        let encrypted = EncryptedGroupOp {
            nonce: [42u8; 12],
            ciphertext: vec![1, 2, 3, 4],
        };

        let op = SignedNamespaceOp::sign(
            &sk,
            sample_namespace_id(),
            vec![],
            [0u8; 32],
            1,
            NamespaceOp::Group {
                group_id: sample_group_id(),
                encrypted,
            },
        )
        .expect("sign");

        op.verify_signature().expect("verify");
        assert_eq!(op.group_id(), Some(sample_group_id()));
    }

    #[test]
    fn namespace_op_tampered_fails() {
        let mut rng = OsRng;
        let sk = PrivateKey::random(&mut rng);

        let mut op = SignedNamespaceOp::sign(
            &sk,
            sample_namespace_id(),
            vec![],
            [0u8; 32],
            1,
            NamespaceOp::Root(RootOp::AdminChanged {
                new_admin: sk.public_key(),
            }),
        )
        .expect("sign");

        op.nonce = 999;
        assert!(op.verify_signature().is_err());
    }

    #[test]
    fn namespace_op_content_hash_distinct() {
        let mut rng = OsRng;
        let sk = PrivateKey::random(&mut rng);

        let op1 = SignedNamespaceOp::sign(
            &sk,
            sample_namespace_id(),
            vec![],
            [0u8; 32],
            1,
            NamespaceOp::Root(RootOp::GroupCreated {
                group_id: sample_group_id(),
            }),
        )
        .expect("sign");

        let op2 = SignedNamespaceOp::sign(
            &sk,
            sample_namespace_id(),
            vec![],
            [0u8; 32],
            2,
            NamespaceOp::Root(RootOp::GroupCreated {
                group_id: sample_group_id(),
            }),
        )
        .expect("sign");

        assert_ne!(
            op1.content_hash().unwrap(),
            op2.content_hash().unwrap(),
            "different nonces must yield different content hashes"
        );
    }

    #[test]
    fn namespace_signable_bytes_deterministic() {
        let mut rng = OsRng;
        let sk = PrivateKey::random(&mut rng);
        let pk = sk.public_key();
        let s = SignableNamespaceOp {
            version: SIGNED_NAMESPACE_OP_SCHEMA_VERSION,
            namespace_id: sample_namespace_id(),
            parent_op_hashes: vec![],
            state_hash: [0u8; 32],
            signer: pk,
            nonce: 42,
            op: NamespaceOp::Root(RootOp::GroupCreated {
                group_id: sample_group_id(),
            }),
        };
        let a = namespace_signable_bytes(&s).expect("bytes");
        let b = namespace_signable_bytes(&s).expect("bytes");
        assert_eq!(a, b);
        assert!(a.starts_with(NAMESPACE_GOVERNANCE_SIGN_DOMAIN));
    }
}
