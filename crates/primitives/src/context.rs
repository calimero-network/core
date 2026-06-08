// We get this warning as we allow to reimport self crate (`calimero_primitives`) inside the tests with a `borsh` feature.
#![cfg_attr(test, allow(unused_extern_crates))]

use core::fmt::{self, Display};
use core::ops::Deref;
use core::str::FromStr;
use std::io;

use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;

use crate::application::ApplicationId;
use crate::common::DIGEST_SIZE;
use crate::hash::{Hash, HashError};

/// A unique identifier for a Context, derived from a cryptographic hash.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, PartialOrd, Ord)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshDeserialize, borsh::BorshSerialize)
)]
// todo! define macros that construct newtypes
// todo! wrapping Hash<N> with this interface
pub struct ContextId(Hash);

impl From<[u8; DIGEST_SIZE]> for ContextId {
    fn from(id: [u8; DIGEST_SIZE]) -> Self {
        Self(id.into())
    }
}

impl AsRef<[u8; DIGEST_SIZE]> for ContextId {
    fn as_ref(&self) -> &[u8; DIGEST_SIZE] {
        &self.0
    }
}

impl Deref for ContextId {
    type Target = [u8; DIGEST_SIZE];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl ContextId {
    /// Creates a special ContextID that contains all zeroes inside.
    ///
    /// This is useful as some modules use the zero context for special functions.
    /// For example, when the new identity is created and it's not assigned to any specific
    /// context, it's associated with the zero context.
    #[must_use]
    pub fn zero() -> Self {
        Self::from([0_u8; DIGEST_SIZE])
    }

    // Returns ContextID represented as a 32-byte array.
    pub fn digest(&self) -> &[u8; DIGEST_SIZE] {
        &self.0
    }
}

impl fmt::Display for ContextId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl From<ContextId> for String {
    fn from(id: ContextId) -> Self {
        id.0.to_base58()
    }
}

impl From<&ContextId> for String {
    fn from(id: &ContextId) -> Self {
        id.0.to_base58()
    }
}

#[derive(Clone, Copy, Debug, ThisError)]
#[error(transparent)]
pub struct InvalidContextId(HashError);

impl FromStr for ContextId {
    type Err = InvalidContextId;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.parse().map_err(InvalidContextId)?))
    }
}

/// Represents the core metadata of a Context.
///
/// This struct provides a snapshot of the context's essential properties,
/// including its unique ID, the application it's running, its current state's root hash,
/// and the current DAG heads for causal delta tracking.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Context {
    /// The unique identifier for this context.
    pub id: ContextId,
    /// The identifier of the application logic running within this context.
    pub application_id: ApplicationId,
    /// Which service from the application bundle this context runs.
    /// None for single-service applications (backward compat).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub service_name: Option<String>,
    /// The root hash of the context's state Merkle tree.
    // Explicit rename (overrides struct-level rename_all = "camelCase")
    // pins the public JSON name to `contextStateHash` independent of the
    // Rust field name. Part of the cross-DAG auth roadmap's three-level
    // naming pattern: contextStateHash / groupStateHash / namespaceStateHash.
    #[serde(rename = "contextStateHash")]
    pub root_hash: Hash,
    /// Current DAG heads (delta IDs with no children yet)
    /// Used to track causal dependencies when creating new deltas
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub dag_heads: Vec<[u8; 32]>,
    /// Resolved semver of the application this context runs (from
    /// `ApplicationMeta.version`). Lets a frontend detect bundle skew in one
    /// call. `None` when the application row is unavailable. Optional +
    /// serde-default so older payloads deserialize unchanged.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub application_version: Option<String>,
}

impl Context {
    /// Constructs a new `Context`.
    ///
    /// # Arguments
    ///
    /// * `id` - The unique `ContextId`.
    /// * `application_id` - The `ApplicationId` for the context.
    /// * `root_hash` - The initial state `Hash`.
    #[must_use]
    pub const fn new(id: ContextId, application_id: ApplicationId, root_hash: Hash) -> Self {
        Self {
            id,
            application_id,
            service_name: None,
            root_hash,
            dag_heads: Vec::new(),
            application_version: None,
        }
    }

    /// Constructs a new `Context` with DAG heads.
    #[must_use]
    pub const fn with_dag_heads(
        id: ContextId,
        application_id: ApplicationId,
        root_hash: Hash,
        dag_heads: Vec<[u8; 32]>,
    ) -> Self {
        Self {
            id,
            application_id,
            service_name: None,
            root_hash,
            dag_heads,
            application_version: None,
        }
    }

    /// Constructs a new `Context` with an optional service name.
    #[must_use]
    pub fn with_service(
        id: ContextId,
        application_id: ApplicationId,
        root_hash: Hash,
        dag_heads: Vec<[u8; 32]>,
        service_name: Option<String>,
    ) -> Self {
        Self {
            id,
            application_id,
            service_name,
            root_hash,
            dag_heads,
            application_version: None,
        }
    }

    /// Sets the resolved application semver (builder-style; `Context` is
    /// `#[non_exhaustive]`, so callers in other crates set it via this method).
    #[must_use]
    pub fn with_application_version(mut self, application_version: Option<String>) -> Self {
        self.application_version = application_version;
        self
    }
}

/// A collection of configuration parameters for a Context.
#[derive(Clone, Debug)]
pub struct ContextConfigParams {
    /// The application that this context runs, supplied by the caller during
    /// bootstrap so `sync_context_config` does not need to read `ContextMeta`
    /// (which has not been written yet at that point).
    pub application_id: Option<ApplicationId>,
    /// A revision number for the application, used for tracking updates.
    pub application_revision: u64,
    /// A revision number for the members list, used for tracking membership changes.
    pub members_revision: u64,
    /// Which service from the application bundle this context runs.
    /// None for single-service applications.
    pub service_name: Option<String>,
}

/// Controls how application upgrades propagate across contexts in a group.
///
/// A migration-carrying upgrade is only valid under `LazyOnAccess` (receivers
/// run the migrate on next access); `Automatic` is for code-only upgrades. The
/// former `Coordinated` variant (borsh tag `2`) was removed — it did nothing
/// `Automatic` didn't and its `deadline` was never enforced. Tag `2` is now
/// rejected on deserialize (see the borsh impl below).
///
/// `#[non_exhaustive]` is retained deliberately even though only two variants
/// remain: future policies (e.g. an eager migrating `Automatic`) can be added
/// without it being a breaking change for downstream matchers. Any new variant
/// MUST also be assigned a borsh tag in the manual impl below.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum UpgradePolicy {
    /// Upgrade all contexts immediately when the group target changes.
    Automatic,
    /// Upgrade each context transparently on its next execution.
    #[default]
    LazyOnAccess,
}

#[cfg(feature = "borsh")]
const _: () = {
    use borsh::{BorshDeserialize, BorshSerialize};
    use std::io::{Read, Write};

    // Tags are stable wire/storage identifiers: 0 = Automatic, 1 = LazyOnAccess,
    // 2 = removed `Coordinated` (now rejected). A new variant must claim the
    // next free tag here AND add its arm to the deserializer below.
    impl BorshSerialize for UpgradePolicy {
        fn serialize<W: Write>(&self, writer: &mut W) -> io::Result<()> {
            match self {
                Self::Automatic => BorshSerialize::serialize(&0u8, writer),
                Self::LazyOnAccess => BorshSerialize::serialize(&1u8, writer),
            }
        }
    }

    impl BorshDeserialize for UpgradePolicy {
        fn deserialize_reader<R: Read>(reader: &mut R) -> io::Result<Self> {
            let tag = u8::deserialize_reader(reader)?;
            match tag {
                0 => Ok(Self::Automatic),
                1 => Ok(Self::LazyOnAccess),
                // Tag 2 was the removed `Coordinated` policy. Reject it
                // explicitly so a stored/in-flight value surfaces loudly
                // instead of being silently reinterpreted.
                2 => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "UpgradePolicy::Coordinated (tag 2) has been removed; \
                     re-set the group's upgrade policy to Automatic or LazyOnAccess",
                )),
                _ => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid UpgradePolicy tag",
                )),
            }
        }
    }
};

/// Distinguishes admin vs regular member within a context group.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshDeserialize, borsh::BorshSerialize)
)]
pub enum GroupMemberRole {
    Admin,
    Member,
    ReadOnly,
    /// Read-only TEE fleet node admitted via hardware attestation.
    ReadOnlyTee,
}

/// A serialized and encoded payload for inviting a user to join a Context Group.
///
/// Internally Borsh-serialized for compact, deterministic representation and
/// then Base58-encoded for a human-readable string format.
/// Supports both targeted invitations (specific invitee) and open invitations (anyone can redeem).
#[derive(Clone, Serialize, Deserialize)]
#[serde(into = "String", try_from = "&str")]
pub struct GroupInvitationPayload(Vec<u8>);

impl fmt::Debug for GroupInvitationPayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("GroupInvitationPayload");
        _ = d.field("raw", &self.to_string());
        d.finish()
    }
}

impl fmt::Display for GroupInvitationPayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&bs58::encode(self.0.as_slice()).into_string())
    }
}

impl FromStr for GroupInvitationPayload {
    type Err = io::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        bs58::decode(s)
            .into_vec()
            .map(Self)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
    }
}

impl From<GroupInvitationPayload> for String {
    fn from(payload: GroupInvitationPayload) -> Self {
        bs58::encode(payload.0.as_slice()).into_string()
    }
}

impl TryFrom<&str> for GroupInvitationPayload {
    type Error = io::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.parse()
    }
}

#[cfg(feature = "borsh")]
const _: () = {
    use borsh::{BorshDeserialize, BorshSerialize};

    use crate::identity::PublicKey;

    #[derive(BorshSerialize, BorshDeserialize)]
    struct GroupInvitationInner {
        group_id: [u8; DIGEST_SIZE],
        inviter_identity: [u8; DIGEST_SIZE],
        invitee_identity: Option<[u8; DIGEST_SIZE]>,
        expiration: Option<u64>,
        inviter_signature: String,
        secret_salt: [u8; 32],
        expiration_timestamp: u64,
    }

    impl GroupInvitationPayload {
        /// Creates a new, serialized group invitation payload.
        #[allow(clippy::too_many_arguments)]
        pub fn new(
            group_id: [u8; DIGEST_SIZE],
            inviter_identity: PublicKey,
            invitee_identity: Option<PublicKey>,
            expiration: Option<u64>,
            inviter_signature: String,
            secret_salt: [u8; 32],
            expiration_timestamp: u64,
        ) -> io::Result<Self> {
            let payload = GroupInvitationInner {
                group_id,
                inviter_identity: *inviter_identity,
                invitee_identity: invitee_identity.map(|pk| *pk),
                expiration,
                inviter_signature,
                secret_salt,
                expiration_timestamp,
            };

            borsh::to_vec(&payload).map(Self)
        }

        /// Deserializes the payload and extracts its constituent parts.
        #[allow(clippy::type_complexity)]
        pub fn parts(
            &self,
        ) -> io::Result<(
            [u8; DIGEST_SIZE],
            PublicKey,
            Option<PublicKey>,
            Option<u64>,
            String,
            [u8; 32],
            u64,
        )> {
            let payload: GroupInvitationInner = borsh::from_slice(&self.0)?;

            Ok((
                payload.group_id,
                payload.inviter_identity.into(),
                payload.invitee_identity.map(Into::into),
                payload.expiration,
                payload.inviter_signature,
                payload.secret_salt,
                payload.expiration_timestamp,
            ))
        }
    }
};

#[cfg(test)]
mod tests {
    use super::*;

    use crate::common::DIGEST_SIZE;
    use crate::identity::PublicKey;

    use std::str::FromStr;

    #[test]
    fn test_context_id_rountrip() {
        // Create context id
        let context_id = ContextId::from([1; DIGEST_SIZE]);
        let encoded_context_id = context_id.to_string();

        // Verify context id is an expected one
        let expected_encoded_context_id = "4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi";
        assert!(!encoded_context_id.is_empty());
        assert_eq!(&encoded_context_id, expected_encoded_context_id);
    }

    #[test]
    fn test_context_id_invalid_base58() {
        // Try to decode an invalid context_id
        let invalid_encoded_context_id = "Invalid!";
        let result = ContextId::from_str(invalid_encoded_context_id);
        assert!(matches!(
            result,
            Err(InvalidContextId(HashError::DecodeError(_)))
        ));
    }

    #[test]
    fn test_group_invitation_payload_roundtrip_targeted() {
        let group_id = [3u8; DIGEST_SIZE];
        let inviter = PublicKey::from([4; DIGEST_SIZE]);
        let invitee = PublicKey::from([5; DIGEST_SIZE]);
        let salt = [9u8; 32];

        let payload = GroupInvitationPayload::new(
            group_id,
            inviter,
            Some(invitee),
            Some(1_700_000_000),
            "abcd1234".to_string(),
            salt,
            999_999_999,
        )
        .expect("Payload creation should succeed");

        let encoded = payload.to_string();
        assert!(!encoded.is_empty());

        let decoded =
            GroupInvitationPayload::from_str(&encoded).expect("Payload decoding should succeed");

        let (g, inv, invitee_out, exp, sig, decoded_salt, exp_ts) =
            decoded.parts().expect("Parts extraction should succeed");
        assert_eq!(g, group_id);
        assert_eq!(inv, inviter);
        assert_eq!(invitee_out, Some(invitee));
        assert_eq!(exp, Some(1_700_000_000));
        assert_eq!(sig, "abcd1234");
        assert_eq!(decoded_salt, salt);
        assert_eq!(exp_ts, 999_999_999);
    }

    #[test]
    fn test_group_invitation_payload_roundtrip_open() {
        let group_id = [6u8; DIGEST_SIZE];
        let inviter = PublicKey::from([7; DIGEST_SIZE]);
        let salt = [10u8; 32];

        let payload = GroupInvitationPayload::new(
            group_id,
            inviter,
            None,
            None,
            "sig_hex".to_string(),
            salt,
            1_000_000_000,
        )
        .expect("Payload creation should succeed");

        let encoded = payload.to_string();
        let decoded =
            GroupInvitationPayload::from_str(&encoded).expect("Payload decoding should succeed");

        let (g, inv, invitee_out, exp, sig, decoded_salt, exp_ts) =
            decoded.parts().expect("Parts extraction should succeed");
        assert_eq!(g, group_id);
        assert_eq!(inv, inviter);
        assert_eq!(invitee_out, None);
        assert_eq!(exp, None);
        assert_eq!(sig, "sig_hex");
        assert_eq!(decoded_salt, salt);
        assert_eq!(exp_ts, 1_000_000_000);
    }

    #[test]
    fn test_group_invitation_payload_invalid_base58() {
        let result = GroupInvitationPayload::from_str("This is not valid Base58!");
        assert!(result.is_err());
    }

    #[cfg(feature = "borsh")]
    #[test]
    fn upgrade_policy_borsh_roundtrip() {
        for policy in [UpgradePolicy::Automatic, UpgradePolicy::LazyOnAccess] {
            let bytes = borsh::to_vec(&policy).expect("serialize");
            let decoded: UpgradePolicy = borsh::from_slice(&bytes).expect("deserialize");
            assert_eq!(decoded, policy);
        }
    }

    #[cfg(feature = "borsh")]
    #[test]
    fn legacy_coordinated_policy_tag_is_rejected() {
        // Borsh encoding of the now-removed `Coordinated { deadline:
        // Some(3600s) }` policy: tag `2`, then `Option::Some` (`1`), then the
        // `(secs: u64, nanos: u32)` tuple little-endian (3600 = 0x0E10, 0).
        // `Coordinated` has been removed, so this previously-valid value must
        // now fail to decode rather than silently round-tripping.
        let legacy_coordinated = [2u8, 1, 0x10, 0x0E, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let result = borsh::from_slice::<UpgradePolicy>(&legacy_coordinated);
        assert!(
            result.is_err(),
            "legacy Coordinated (tag 2) must be rejected, got {result:?}"
        );
    }
}
