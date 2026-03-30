//! Signed group operations for **local** governance (no chain).
//!
//! See `docs/context-management/LOCAL-GROUP-GOVERNANCE.md`.

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
    /// Per-context visibility and creator pubkey.
    ContextVisibilitySet {
        context_id: ContextId,
        /// `0` = Open, `1` = Restricted.
        mode: u8,
        creator: PublicKey,
    },
    /// Replace the full allowlist for a restricted context.
    ContextAllowlistReplaced {
        context_id: ContextId,
        members: Vec<PublicKey>,
    },
    /// Human-readable alias for a context within the group.
    /// **Signer:** group admin, or the context creator (must match `GroupContextVisibility.creator`).
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
    /// Join a group using an admin-signed open invitation plus joiner proof (see `join_group`).
    JoinWithInvitationClaim {
        signed_invitation: SignedGroupOpenInvitation,
        invitee_signature_hex: String,
    },
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
    /// Link a child group under this group (admin only).
    /// Published on the parent group's gossip topic.
    SubgroupCreated { child_group_id: [u8; 32] },
    /// Unlink a child group from this group (admin only).
    /// Does not delete the child group or its members/contexts.
    SubgroupRemoved { child_group_id: [u8; 32] },
    /// Join a group via a context-level open invitation.
    /// The inviter signature proves an admin created the invitation;
    /// the outer `SignedGroupOp` signature proves the joiner's identity.
    MemberJoinedViaContextInvitation {
        /// Context ID from the invitation.
        context_id: ContextId,
        /// The inviter's public key (must be a group admin).
        inviter_id: PublicKey,
        /// Borsh-serialized `InvitationFromMember` (needed for signature verification).
        invitation_payload: Vec<u8>,
        /// Hex-encoded inviter signature over the invitation payload.
        inviter_signature: String,
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
}
