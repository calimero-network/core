// We get this warning as we allow to reimport self crate (`calimero_primitives`) inside the tests with a `borsh` feature.
#![cfg_attr(test, allow(unused_extern_crates))]

use core::fmt;
use core::ops::Deref;
use core::str::FromStr;
use std::borrow::Cow;
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
    pub fn digest(&self) -> &[u8; 32] {
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

/// A serialized and encoded payload for inviting a user to join a Context.
///
/// It is internally Borsh-serialized for compact, deterministic representation and
/// then Base58-encoded for a human-readable string format.
#[derive(Clone, Serialize, Deserialize)]
#[serde(into = "String", try_from = "&str")]
pub struct ContextInvitationPayload(Vec<u8>);

impl fmt::Debug for ContextInvitationPayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        #[cfg(feature = "borsh")]
        {
            let is_alternate = f.alternate();
            let mut d = f.debug_struct("ContextInvitationPayload");
            let (context_id, invitee_id, protocol, network, contract_id) =
                self.parts().map_err(|_| fmt::Error)?;

            _ = d
                .field("context_id", &context_id)
                .field("invitee_id", &invitee_id)
                .field("protocol", &protocol)
                .field("network", &network)
                .field("contract_id", &contract_id);

            if !is_alternate {
                return d.finish();
            }
        }

        let mut d = f.debug_struct("ContextInvitationPayload");
        _ = d.field("raw", &self.to_string());

        d.finish()
    }
}

impl fmt::Display for ContextInvitationPayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&bs58::encode(self.0.as_slice()).into_string())
    }
}

impl FromStr for ContextInvitationPayload {
    type Err = io::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        bs58::decode(s)
            .into_vec()
            .map(Self)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
    }
}

impl From<ContextInvitationPayload> for String {
    fn from(payload: ContextInvitationPayload) -> Self {
        bs58::encode(payload.0.as_slice()).into_string()
    }
}

impl TryFrom<&str> for ContextInvitationPayload {
    type Error = io::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.parse()
    }
}

// TODO: add implementation of `impl TryFrom<InvitationPayload> for ContextInvitationPayload` and vice
// versa for more convenient handling.

#[cfg(feature = "borsh")]
#[expect(single_use_lifetimes, reason = "False positive")]
const _: () = {
    use std::borrow::Cow;

    use borsh::{BorshDeserialize, BorshSerialize};

    use crate::identity::PublicKey;

    #[derive(BorshSerialize, BorshDeserialize)]
    struct InvitationPayload<'a> {
        context_id: [u8; DIGEST_SIZE],
        invitee_id: [u8; DIGEST_SIZE],
        protocol: Cow<'a, str>,
        network: Cow<'a, str>,
        contract_id: Cow<'a, str>,
    }

    /// Creates a new, serialized invitation payload.
    ///
    /// # Arguments
    /// * `context_id` - The ID of the context to join.
    /// * `invitee_id` - The public key of the identity being invited.
    /// * `protocol` - The protocol used by the context (e.g. "near").
    /// * `network` - The network identifier (e.g. "testnet").
    /// * `contract_id` - The contract id for the context.
    ///
    /// # Returns
    /// A `Result` containing the `ContextInvitationPayload` or an `io::Error` if serialization fails.
    impl ContextInvitationPayload {
        pub fn new(
            context_id: ContextId,
            invitee_id: PublicKey,
            protocol: Cow<'_, str>,
            network: Cow<'_, str>,
            contract_id: Cow<'_, str>,
        ) -> io::Result<Self> {
            let payload = InvitationPayload {
                context_id: *context_id,
                invitee_id: *invitee_id,
                protocol,
                network,
                contract_id,
            };

            borsh::to_vec(&payload).map(Self)
        }

        /// Deserializes the payload and extracts its constituent parts.
        ///
        /// # Returns
        /// A `Result` containing a tuple of the decoded parts or an `io::Error` if deserialization fails.
        /// The returned tuple consists of: `context_id, public_key, protocol, network, contract_id`.
        pub fn parts(&self) -> io::Result<(ContextId, PublicKey, String, String, String)> {
            let payload: InvitationPayload<'_> = borsh::from_slice(&self.0)?;

            Ok((
                payload.context_id.into(),
                payload.invitee_id.into(),
                payload.protocol.into_owned(),
                payload.network.into_owned(),
                payload.contract_id.into_owned(),
            ))
        }
    }
};

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
            root_hash,
            dag_heads,
        }
    }
}

/// A collection of configuration parameters for a Context.
///
/// This is used to define the external, on-chain properties of a context.
/// The use of `Cow<'a, str>` is an optimization to avoid string allocations when the
/// data can be borrowed.
#[derive(Clone, Debug)]
pub struct ContextConfigParams<'a> {
    /// The name of the protocol used for external communication (e.g., "near").
    pub protocol: Cow<'a, str>,
    /// The identifier of the external network (e.g., "testnet").
    pub network_id: Cow<'a, str>,
    /// The account ID of the main smart contract for this context on the external network.
    pub contract_id: Cow<'a, str>,
    /// The account ID of a proxy contract used for interactions.
    pub proxy_contract: Cow<'a, str>,
    /// A revision number for the application, used for tracking updates.
    pub application_revision: u64,
    /// A revision number for the members list, used for tracking membership changes.
    pub members_revision: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::common::DIGEST_SIZE;
    use crate::identity::PublicKey;

    use std::str::FromStr;

    #[test]
    fn test_context_invitation_payload_roundtrip() {
        // Setup initial data
        let context_id = ContextId::from([1; DIGEST_SIZE]);
        let invitee_id = PublicKey::from([2; DIGEST_SIZE]);
        let protocol = String::from("near");
        let network = String::from("testnet");
        let contract_id = String::from("calimero.testnet");

        // Create the context invitation payload
        let invitation_payload = ContextInvitationPayload::new(
            context_id,
            invitee_id,
            protocol.clone().into(),
            network.clone().into(),
            contract_id.clone().into(),
        )
        .expect("Payload creation should succeed");

        // Encode context invitation payload to a Base58 string
        let encoded_string = invitation_payload.to_string();
        let expected_encoded_string = "4Mb2deLtaS7ApRdhh5ms1GuT2GtGKkKxDMakmd1C3YA2NRYwniNiijac1Nn8NYHVeWoqzvYFpmJMekAeUYKyWpq5j1QaVjpW6r5V86oo3tz6uKx1Hri82LHf8rrs2m2dyjZUZeCaeBEb";
        assert!(!encoded_string.is_empty());
        assert_eq!(&encoded_string, expected_encoded_string);

        // Decode context invitation payload back from the Base58 string
        let decoded_invitation_payload = ContextInvitationPayload::from_str(&encoded_string)
            .expect("Payload decoding should succeed");

        // Extract parts and verify they match the original data
        let (
            decoded_context_id,
            decoded_invitee_id,
            decoded_protocol,
            decoded_network,
            decoded_contract_id,
        ) = decoded_invitation_payload
            .parts()
            .expect("Extracting parts should succeed");

        assert_eq!(context_id, decoded_context_id);
        assert_eq!(invitee_id, decoded_invitee_id);
        assert_eq!(protocol, decoded_protocol);
        assert_eq!(network, decoded_network);
        assert_eq!(contract_id, decoded_contract_id);
    }

    #[test]
    fn test_context_invitation_payload_invalid_base58() {
        let invalid_str = "This is not valid Base58!";
        let result = ContextInvitationPayload::from_str(invalid_str);
        assert!(result.is_err());
    }

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
}
