use std::collections::BTreeMap;

use candid::{Decode, Encode, Principal};

use crate::client::env::config::requests::{
    ApplicationRequest, ApplicationRevisionRequest, FetchNonceRequest, HasMemberRequest,
    MembersRequest, MembersRevisionRequest, PrivilegesRequest, ProxyContractRequest,
};
use crate::client::env::Method;
use crate::client::protocol::icp::Icp;
use crate::icp::repr::ICRepr;
use crate::icp::types::{ICApplication, ICCapability};
use crate::repr::ReprTransmute;
use crate::types::{Application, Capability, ContextIdentity, Revision, SignerId};

impl Method<Icp> for ApplicationRequest {
    type Returns = Application<'static>;

    const METHOD: &'static str = "application";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id = ICRepr::new(self.context_id);
        Encode!(&context_id).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let decoded = Decode!(&response, ICApplication)?;
        Ok(decoded.into())
    }
}

impl Method<Icp> for ApplicationRevisionRequest {
    type Returns = u64;

    const METHOD: &'static str = "application_revision";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id = ICRepr::new(*self.context_id);
        Encode!(&context_id).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        Decode!(&response, Revision).map_err(Into::into)
    }
}

impl Method<Icp> for MembersRequest {
    type Returns = Vec<ContextIdentity>;

    const METHOD: &'static str = "members";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id = ICRepr::new(*self.context_id);

        Encode!(&context_id, &self.offset, &self.length).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let members = Decode!(&response, Vec<ICRepr<ContextIdentity>>)?;

        // safety: `ICRepr<T>` is a transparent wrapper around `T`
        #[expect(
            clippy::transmute_undefined_repr,
            reason = "ICRepr<T> is a transparent wrapper around T"
        )]
        let members = unsafe {
            std::mem::transmute::<Vec<ICRepr<ContextIdentity>>, Vec<ContextIdentity>>(members)
        };

        Ok(members)
    }
}

impl Method<Icp> for HasMemberRequest {
    type Returns = bool;

    const METHOD: &'static str = "has_member";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let mut encoded = Vec::new();

        let context_raw: [u8; 32] = self
            .context_id
            .rt()
            .map_err(|e| eyre::eyre!("cannot convert context id to raw bytes: {}", e))?;
        encoded.extend_from_slice(&context_raw);

        let member_raw: [u8; 32] = self
            .identity
            .rt()
            .map_err(|e| eyre::eyre!("cannot convert identity to raw bytes: {}", e))?;
        encoded.extend_from_slice(&member_raw);

        Ok(encoded)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let value = Decode!(&response, Self::Returns)?;
        Ok(value)
    }
}

impl Method<Icp> for MembersRevisionRequest {
    type Returns = Revision;

    const METHOD: &'static str = "members_revision";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id = ICRepr::new(*self.context_id);
        Encode!(&context_id).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let value = Decode!(&response, Self::Returns)?;
        Ok(value)
    }
}

impl<'a> Method<Icp> for PrivilegesRequest<'a> {
    type Returns = BTreeMap<SignerId, Vec<Capability>>;

    const METHOD: &'static str = "privileges";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id = ICRepr::new(*self.context_id);

        // safety:
        //  `Repr<T>` is a transparent wrapper around `T` and
        //  `ICRepr<T>` is a transparent wrapper around `T`

        let identities = unsafe {
            &*(std::ptr::from_ref::<[crate::repr::Repr<ContextIdentity>]>(self.identities)
                as *const [ICRepr<ContextIdentity>])
        };

        let payload = (context_id, identities);

        Encode!(&payload).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let decoded = Decode!(&response, BTreeMap<ICRepr<SignerId>, Vec<ICCapability>>)?;

        Ok(decoded
            .into_iter()
            .map(|(k, v)| (*k, v.into_iter().map(Into::into).collect()))
            .collect())
    }
}

impl Method<Icp> for ProxyContractRequest {
    const METHOD: &'static str = "proxy_contract";

    type Returns = String;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id = ICRepr::new(*self.context_id);
        Encode!(&context_id).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let value: Principal = Decode!(&response, Principal)?;
        let value_as_string = value.to_text();
        Ok(value_as_string)
    }
}

impl Method<Icp> for FetchNonceRequest {
    type Returns = Option<u64>;

    const METHOD: &'static str = "fetch_nonce";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let context_id = ICRepr::new(*self.context_id);
        let member_id = ICRepr::new(*self.member_id);

        // Encode arguments separately
        Encode!(&context_id, &member_id).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let decoded = Decode!(&response, Option<u64>)?;

        Ok(decoded)
    }
}
