// We get this warning as we allow to reimport self crate (`calimero_primitives`) inside the tests with a `borsh` feature.
#![cfg_attr(test, allow(unused_extern_crates))]

use core::fmt;
use core::ops::Deref;
use core::str::FromStr;
use core::time::Duration;
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
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

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
        f.pad(self.as_str())
    }
}

impl From<ContextId> for String {
    fn from(id: ContextId) -> Self {
        id.as_str().to_owned()
    }
}

impl From<&ContextId> for String {
    fn from(id: &ContextId) -> Self {
        id.as_str().to_owned()
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
    pub root_hash: Hash,
    /// Current DAG heads (delta IDs with no children yet)
    /// Used to track causal dependencies when creating new deltas
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub dag_heads: Vec<[u8; 32]>,
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
        }
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
}

/// Controls how application upgrades propagate across contexts in a group.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum UpgradePolicy {
    /// Upgrade all contexts immediately when the group target changes.
    Automatic,
    /// Upgrade each context transparently on its next execution.
    #[default]
    LazyOnAccess,
    /// Upgrade all contexts with an optional deadline for completion.
    Coordinated { deadline: Option<Duration> },
}

#[cfg(feature = "borsh")]
const _: () = {
    use borsh::{BorshDeserialize, BorshSerialize};
    use std::io::{Read, Write};

    impl BorshSerialize for UpgradePolicy {
        fn serialize<W: Write>(&self, writer: &mut W) -> io::Result<()> {
            match self {
                Self::Automatic => BorshSerialize::serialize(&0u8, writer),
                Self::LazyOnAccess => BorshSerialize::serialize(&1u8, writer),
                Self::Coordinated { deadline } => {
                    BorshSerialize::serialize(&2u8, writer)?;
                    let dur = deadline.map(|d| (d.as_secs(), d.subsec_nanos()));
                    BorshSerialize::serialize(&dur, writer)
                }
            }
        }
    }

    impl BorshDeserialize for UpgradePolicy {
        fn deserialize_reader<R: Read>(reader: &mut R) -> io::Result<Self> {
            let tag = u8::deserialize_reader(reader)?;
            match tag {
                0 => Ok(Self::Automatic),
                1 => Ok(Self::LazyOnAccess),
                2 => {
                    let dur: Option<(u64, u32)> = BorshDeserialize::deserialize_reader(reader)?;
                    Ok(Self::Coordinated {
                        deadline: dur
                            .map(|(s, n)| -> io::Result<Duration> {
                                if n >= 1_000_000_000 {
                                    return Err(io::Error::new(
                                        io::ErrorKind::InvalidData,
                                        "nanoseconds field exceeds 999_999_999",
                                    ));
                                }
                                Ok(Duration::new(s, n))
                            })
                            .transpose()?,
                    })
                }
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
}
