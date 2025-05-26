use core::fmt;
use core::ops::Deref;
use core::str::FromStr;
use std::borrow::Cow;
use std::io;

use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;

use crate::application::ApplicationId;
use crate::hash::{Hash, HashError};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, PartialOrd, Ord)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshDeserialize, borsh::BorshSerialize)
)]
// todo! define macros that construct newtypes
// todo! wrapping Hash<N> with this interface
pub struct ContextId(Hash);

impl From<[u8; 32]> for ContextId {
    fn from(id: [u8; 32]) -> Self {
        Self(id.into())
    }
}

impl AsRef<[u8; 32]> for ContextId {
    fn as_ref(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Deref for ContextId {
    type Target = [u8; 32];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl ContextId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
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

#[cfg(feature = "borsh")]
#[expect(single_use_lifetimes, reason = "False positive")]
const _: () = {
    use std::borrow::Cow;

    use borsh::{BorshDeserialize, BorshSerialize};

    use crate::identity::PublicKey;

    #[derive(BorshSerialize, BorshDeserialize)]
    struct InvitationPayload<'a> {
        context_id: [u8; 32],
        invitee_id: [u8; 32],
        protocol: Cow<'a, str>,
        network: Cow<'a, str>,
        contract_id: Cow<'a, str>,
    }

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

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Context {
    pub id: ContextId,
    pub application_id: ApplicationId,
    pub root_hash: Hash,
}

impl Context {
    #[must_use]
    pub const fn new(id: ContextId, application_id: ApplicationId, root_hash: Hash) -> Self {
        Self {
            id,
            application_id,
            root_hash,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ContextConfigParams<'a> {
    pub protocol: Cow<'a, str>,
    pub network_id: Cow<'a, str>,
    pub contract_id: Cow<'a, str>,
    pub proxy_contract: Cow<'a, str>,
    pub application_revision: u64,
    pub members_revision: u64,
}
