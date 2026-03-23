//! Prototype governance reducer for signed, deterministic control operations.
//!
//! This module demonstrates a no-quorum control-plane model where:
//! - every operation is user-signed;
//! - nonce checks provide replay protection;
//! - authorization is enforced by a deterministic reducer; and
//! - applied operations are persisted in an immutable (first-write-wins) log.

use std::collections::{BTreeMap, BTreeSet};

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::collections::FrozenValue;

/// Capability bit for member-management operations.
pub const CAP_MANAGE_MEMBERS: u32 = 1 << 0;
/// Capability bit for capability-management operations.
pub const CAP_MANAGE_CAPABILITIES: u32 = 1 << 1;

/// The unsigned governance operation payload.
#[derive(Copy, Clone, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct GovernanceOperation {
    /// Operation signer.
    pub signer: PublicKey,
    /// Replay-protection nonce.
    pub nonce: u64,
    /// Operation kind.
    pub kind: GovernanceOperationKind,
}

impl GovernanceOperation {
    /// Creates a governance operation payload.
    #[must_use]
    pub const fn new(signer: PublicKey, nonce: u64, kind: GovernanceOperationKind) -> Self {
        Self {
            signer,
            nonce,
            kind,
        }
    }
}

/// Supported governance operations.
#[derive(Copy, Clone, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub enum GovernanceOperationKind {
    /// Adds a member.
    AddMember { member: PublicKey },
    /// Removes a member.
    RemoveMember { member: PublicKey },
    /// Grants capability bits to a member.
    GrantCapabilities { member: PublicKey, capabilities: u32 },
    /// Revokes capability bits from a member.
    RevokeCapabilities { member: PublicKey, capabilities: u32 },
    /// Adds an admin.
    AddAdmin { admin: PublicKey },
    /// Removes an admin.
    RemoveAdmin { admin: PublicKey },
}

/// A signed governance operation.
#[derive(Copy, Clone, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct SignedGovernanceOperation {
    /// Signed payload.
    pub operation: GovernanceOperation,
    /// Ed25519 signature over serialized [`GovernanceOperation`].
    pub signature: [u8; 64],
}

impl SignedGovernanceOperation {
    /// Signs a governance operation.
    ///
    /// # Errors
    ///
    /// Returns an error if payload serialization or signing fails.
    pub fn sign(operation: GovernanceOperation, private_key: &PrivateKey) -> Result<Self, Error> {
        let payload = borsh::to_vec(&operation).map_err(|err| Error::Serialization(err.to_string()))?;
        let signature = private_key
            .sign(&payload)
            .map_err(|err| Error::Signing(err.to_string()))?
            .to_bytes();

        Ok(Self {
            operation,
            signature,
        })
    }

    /// Verifies the signature.
    ///
    /// # Errors
    ///
    /// Returns an error if payload serialization or signature verification fails.
    pub fn verify(&self) -> Result<(), Error> {
        let payload = borsh::to_vec(&self.operation)
            .map_err(|err| Error::Serialization(err.to_string()))?;

        self.operation
            .signer
            .verify_raw_signature(&payload, &self.signature)
            .map_err(|err| Error::InvalidSignature(err.to_string()))
    }
}

/// Deterministic governance reducer state.
#[derive(Clone, Debug, Default)]
pub struct GovernanceReducer {
    admins: BTreeSet<PublicKey>,
    members: BTreeSet<PublicKey>,
    capabilities: BTreeMap<PublicKey, u32>,
    nonces: BTreeMap<PublicKey, u64>,
    op_log: BTreeMap<[u8; 32], FrozenValue<SignedGovernanceOperation>>,
}

impl GovernanceReducer {
    /// Creates a new reducer with a bootstrap admin.
    #[must_use]
    pub fn new(bootstrap_admin: PublicKey) -> Self {
        let mut admins = BTreeSet::new();
        let mut members = BTreeSet::new();
        let mut nonces = BTreeMap::new();

        let _ = admins.insert(bootstrap_admin);
        let _ = members.insert(bootstrap_admin);
        let _ = nonces.insert(bootstrap_admin, 0);

        Self {
            admins,
            members,
            capabilities: BTreeMap::new(),
            nonces,
            op_log: BTreeMap::new(),
        }
    }

    /// Applies a signed operation and returns its immutable log hash.
    ///
    /// # Errors
    ///
    /// Returns an error if verification, nonce checks, or authorization fails.
    pub fn apply(&mut self, signed: SignedGovernanceOperation) -> Result<[u8; 32], Error> {
        signed.verify()?;

        let signer = signed.operation.signer;
        let expected_nonce = self.nonces.get(&signer).copied().unwrap_or(0);
        if expected_nonce != signed.operation.nonce {
            return Err(Error::NonceMismatch {
                expected: expected_nonce,
                found: signed.operation.nonce,
            });
        }

        self.authorize(&signed.operation)?;
        self.reduce(&signed.operation)?;

        let _ = self.nonces.insert(signer, expected_nonce.saturating_add(1));

        let log_hash = self.log_hash(&signed)?;
        let _ = self.op_log.entry(log_hash).or_insert(FrozenValue(signed));

        Ok(log_hash)
    }

    /// Returns whether `identity` is an admin.
    #[must_use]
    pub fn is_admin(&self, identity: &PublicKey) -> bool {
        self.admins.contains(identity)
    }

    /// Returns whether `identity` is a member.
    #[must_use]
    pub fn is_member(&self, identity: &PublicKey) -> bool {
        self.members.contains(identity)
    }

    /// Returns capability bits for `identity`.
    #[must_use]
    pub fn capabilities_of(&self, identity: &PublicKey) -> u32 {
        self.capabilities.get(identity).copied().unwrap_or(0)
    }

    /// Returns the next expected nonce for `identity`.
    #[must_use]
    pub fn next_expected_nonce(&self, identity: &PublicKey) -> u64 {
        self.nonces.get(identity).copied().unwrap_or(0)
    }

    /// Returns whether an immutable operation hash exists in the log.
    #[must_use]
    pub fn has_operation(&self, hash: &[u8; 32]) -> bool {
        self.op_log.contains_key(hash)
    }

    fn log_hash(&self, signed: &SignedGovernanceOperation) -> Result<[u8; 32], Error> {
        let bytes =
            borsh::to_vec(signed).map_err(|err| Error::Serialization(err.to_string()))?;
        Ok(Sha256::digest(&bytes).into())
    }

    fn authorize(&self, operation: &GovernanceOperation) -> Result<(), Error> {
        if self.admins.contains(&operation.signer) {
            return Ok(());
        }

        let caps = self.capabilities_of(&operation.signer);
        match operation.kind {
            GovernanceOperationKind::AddMember { .. }
            | GovernanceOperationKind::RemoveMember { .. } => {
                if caps & CAP_MANAGE_MEMBERS != 0 {
                    return Ok(());
                }
            }
            GovernanceOperationKind::GrantCapabilities { .. }
            | GovernanceOperationKind::RevokeCapabilities { .. } => {
                if caps & CAP_MANAGE_CAPABILITIES != 0 {
                    return Ok(());
                }
            }
            GovernanceOperationKind::AddAdmin { .. }
            | GovernanceOperationKind::RemoveAdmin { .. } => {}
        }

        Err(Error::Unauthorized(operation.signer))
    }

    fn reduce(&mut self, operation: &GovernanceOperation) -> Result<(), Error> {
        match operation.kind {
            GovernanceOperationKind::AddMember { member } => {
                let _ = self.members.insert(member);
                let _ = self.nonces.entry(member).or_default();
            }
            GovernanceOperationKind::RemoveMember { member } => {
                if !self.members.remove(&member) {
                    return Err(Error::UnknownMember(member));
                }

                if self.admins.contains(&member) {
                    if self.admins.len() == 1 {
                        return Err(Error::CannotRemoveLastAdmin(member));
                    }

                    let _ = self.admins.remove(&member);
                }

                let _ = self.capabilities.remove(&member);
                let _ = self.nonces.remove(&member);
            }
            GovernanceOperationKind::GrantCapabilities {
                member,
                capabilities,
            } => {
                if !self.members.contains(&member) {
                    return Err(Error::UnknownMember(member));
                }

                let current = self.capabilities_of(&member);
                let _ = self.capabilities.insert(member, current | capabilities);
            }
            GovernanceOperationKind::RevokeCapabilities {
                member,
                capabilities,
            } => {
                if !self.members.contains(&member) {
                    return Err(Error::UnknownMember(member));
                }

                let current = self.capabilities_of(&member);
                let updated = current & !capabilities;
                if updated == 0 {
                    let _ = self.capabilities.remove(&member);
                } else {
                    let _ = self.capabilities.insert(member, updated);
                }
            }
            GovernanceOperationKind::AddAdmin { admin } => {
                let _ = self.admins.insert(admin);
                let _ = self.members.insert(admin);
                let _ = self.nonces.entry(admin).or_default();
            }
            GovernanceOperationKind::RemoveAdmin { admin } => {
                if !self.admins.contains(&admin) {
                    return Err(Error::UnknownAdmin(admin));
                }

                if self.admins.len() == 1 {
                    return Err(Error::CannotRemoveLastAdmin(admin));
                }

                let _ = self.admins.remove(&admin);
            }
        }

        Ok(())
    }
}

/// Governance reducer error.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// Failed to serialize payload for hashing/signing.
    #[error("serialization error: {0}")]
    Serialization(String),
    /// Failed to sign payload.
    #[error("signing error: {0}")]
    Signing(String),
    /// Signature verification failed.
    #[error("invalid signature: {0}")]
    InvalidSignature(String),
    /// Operation nonce did not match expected replay-protection value.
    #[error("nonce mismatch: expected {expected}, found {found}")]
    NonceMismatch {
        /// Expected nonce.
        expected: u64,
        /// Received nonce.
        found: u64,
    },
    /// Signer is not authorized for requested operation.
    #[error("unauthorized signer: {0}")]
    Unauthorized(PublicKey),
    /// Referenced member does not exist.
    #[error("unknown member: {0}")]
    UnknownMember(PublicKey),
    /// Referenced admin does not exist.
    #[error("unknown admin: {0}")]
    UnknownAdmin(PublicKey),
    /// Removal would leave the system without an admin.
    #[error("cannot remove last admin: {0}")]
    CannotRemoveLastAdmin(PublicKey),
}

#[cfg(test)]
mod tests {
    use super::{
        Error, GovernanceOperation, GovernanceOperationKind, GovernanceReducer,
        SignedGovernanceOperation, CAP_MANAGE_CAPABILITIES, CAP_MANAGE_MEMBERS,
    };
    use calimero_primitives::identity::PrivateKey;

    fn key(seed: u8) -> PrivateKey {
        PrivateKey::from([seed; 32])
    }

    #[test]
    fn admin_can_add_member_and_log_is_immutable() {
        let admin_sk = key(1);
        let admin_pk = admin_sk.public_key();
        let member_pk = key(2).public_key();

        let mut reducer = GovernanceReducer::new(admin_pk);

        let op = GovernanceOperation::new(
            admin_pk,
            0,
            GovernanceOperationKind::AddMember { member: member_pk },
        );
        let signed = SignedGovernanceOperation::sign(op, &admin_sk)
            .expect("test operation should be signable");

        let log_hash = reducer
            .apply(signed)
            .expect("authorized operation should succeed");

        assert!(reducer.is_member(&member_pk));
        assert_eq!(reducer.next_expected_nonce(&admin_pk), 1);
        assert!(reducer.has_operation(&log_hash));
    }

    #[test]
    fn unauthorized_member_is_rejected() {
        let admin_sk = key(3);
        let admin_pk = admin_sk.public_key();
        let member_sk = key(4);
        let member_pk = member_sk.public_key();
        let target_pk = key(5).public_key();

        let mut reducer = GovernanceReducer::new(admin_pk);

        let add_member = GovernanceOperation::new(
            admin_pk,
            0,
            GovernanceOperationKind::AddMember { member: member_pk },
        );
        let add_member = SignedGovernanceOperation::sign(add_member, &admin_sk)
            .expect("test operation should be signable");
        reducer
            .apply(add_member)
            .expect("admin should be able to add member");

        let unauthorized = GovernanceOperation::new(
            member_pk,
            0,
            GovernanceOperationKind::AddMember { member: target_pk },
        );
        let unauthorized = SignedGovernanceOperation::sign(unauthorized, &member_sk)
            .expect("test operation should be signable");

        let err = reducer
            .apply(unauthorized)
            .expect_err("member without capabilities must be rejected");

        assert!(matches!(err, Error::Unauthorized(pk) if pk == member_pk));
    }

    #[test]
    fn invalid_signature_is_rejected() {
        let admin_sk = key(6);
        let admin_pk = admin_sk.public_key();
        let member_pk = key(7).public_key();

        let mut reducer = GovernanceReducer::new(admin_pk);

        let op = GovernanceOperation::new(
            admin_pk,
            0,
            GovernanceOperationKind::AddMember { member: member_pk },
        );
        let mut signed = SignedGovernanceOperation::sign(op, &admin_sk)
            .expect("test operation should be signable");
        signed.signature[0] ^= 0xFF;

        let err = reducer
            .apply(signed)
            .expect_err("corrupted signature must fail verification");

        assert!(matches!(err, Error::InvalidSignature(_)));
    }

    #[test]
    fn replay_nonce_is_rejected() {
        let admin_sk = key(8);
        let admin_pk = admin_sk.public_key();
        let member_pk = key(9).public_key();

        let mut reducer = GovernanceReducer::new(admin_pk);

        let op = GovernanceOperation::new(
            admin_pk,
            0,
            GovernanceOperationKind::AddMember { member: member_pk },
        );
        let signed = SignedGovernanceOperation::sign(op, &admin_sk)
            .expect("test operation should be signable");

        reducer
            .apply(signed)
            .expect("first use of nonce should succeed");
        let err = reducer
            .apply(signed)
            .expect_err("second use of same nonce must fail");

        assert!(matches!(
            err,
            Error::NonceMismatch {
                expected: 1,
                found: 0
            }
        ));
    }

    #[test]
    fn remove_member_cascades_capability_cleanup() {
        let admin_sk = key(10);
        let admin_pk = admin_sk.public_key();
        let member_pk = key(11).public_key();

        let mut reducer = GovernanceReducer::new(admin_pk);

        let add_member = GovernanceOperation::new(
            admin_pk,
            0,
            GovernanceOperationKind::AddMember { member: member_pk },
        );
        reducer
            .apply(
                SignedGovernanceOperation::sign(add_member, &admin_sk)
                    .expect("test operation should be signable"),
            )
            .expect("add member should succeed");

        let grant = GovernanceOperation::new(
            admin_pk,
            1,
            GovernanceOperationKind::GrantCapabilities {
                member: member_pk,
                capabilities: CAP_MANAGE_MEMBERS | CAP_MANAGE_CAPABILITIES,
            },
        );
        reducer
            .apply(
                SignedGovernanceOperation::sign(grant, &admin_sk)
                    .expect("test operation should be signable"),
            )
            .expect("grant should succeed");

        assert_ne!(reducer.capabilities_of(&member_pk), 0);

        let remove_member = GovernanceOperation::new(
            admin_pk,
            2,
            GovernanceOperationKind::RemoveMember { member: member_pk },
        );
        reducer
            .apply(
                SignedGovernanceOperation::sign(remove_member, &admin_sk)
                    .expect("test operation should be signable"),
            )
            .expect("remove member should succeed");

        assert!(!reducer.is_member(&member_pk));
        assert_eq!(reducer.capabilities_of(&member_pk), 0);
        assert_eq!(reducer.next_expected_nonce(&member_pk), 0);
    }
}
