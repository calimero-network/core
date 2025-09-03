use core::mem;
use std::collections::BTreeMap;

use serde::Serialize;

use crate::client::env::config::query::{
    ApplicationRequest, ApplicationRevisionRequest, FetchNonceRequest, HasMemberRequest,
    MembersRequest, MembersRevisionRequest, PrivilegesRequest, ProxyContractRequest,
};
use crate::client::env::Method;
use crate::client::protocol::near::Near;
use crate::repr::{Repr, ReprTransmute};
use crate::types::{Application, Capability, ContextIdentity, Revision, SignerId};

impl Method<Near> for ApplicationRequest {
    const METHOD: &'static str = "application";

    type Returns = Application<'static>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let application: Application<'_> = serde_json::from_slice(&response)?;

        Ok(Application::new(
            application.id,
            application.blob,
            application.size,
            crate::types::ApplicationSource(application.source.0.into_owned().into()),
            crate::types::ApplicationMetadata(Repr::new(
                application.metadata.0.into_inner().into_owned().into(),
            )),
        ))
    }
}

impl Method<Near> for ApplicationRevisionRequest {
    const METHOD: &'static str = "application_revision";

    type Returns = Revision;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        serde_json::from_slice(&response).map_err(Into::into)
    }
}

impl Method<Near> for MembersRequest {
    const METHOD: &'static str = "members";

    type Returns = Vec<ContextIdentity>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let members: Vec<Repr<ContextIdentity>> = serde_json::from_slice(&response)?;

        // safety: `Repr<T>` is a transparent wrapper around `T`
        #[expect(
            clippy::transmute_undefined_repr,
            reason = "Repr<T> is a transparent wrapper around T"
        )]
        let members =
            unsafe { mem::transmute::<Vec<Repr<ContextIdentity>>, Vec<ContextIdentity>>(members) };

        Ok(members)
    }
}

impl Method<Near> for HasMemberRequest {
    const METHOD: &'static str = "has_member";

    type Returns = bool;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        serde_json::from_slice(&response).map_err(Into::into)
    }
}

impl Method<Near> for MembersRevisionRequest {
    const METHOD: &'static str = "members_revision";

    type Returns = Revision;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        serde_json::from_slice(&response).map_err(Into::into)
    }
}

impl<'a> Method<Near> for PrivilegesRequest<'a> {
    const METHOD: &'static str = "privileges";

    type Returns = BTreeMap<SignerId, Vec<Capability>>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let privileges: BTreeMap<Repr<SignerId>, Vec<Capability>> =
            serde_json::from_slice(&response)?;

        // safety: `Repr<T>` is a transparent wrapper around `T`
        let privileges = unsafe {
            #[expect(
                clippy::transmute_undefined_repr,
                reason = "Repr<T> is a transparent wrapper around T"
            )]
            mem::transmute::<
                BTreeMap<Repr<SignerId>, Vec<Capability>>,
                BTreeMap<SignerId, Vec<Capability>>,
            >(privileges)
        };

        Ok(privileges)
    }
}

impl Method<Near> for ProxyContractRequest {
    const METHOD: &'static str = "proxy_contract";

    type Returns = String;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        serde_json::from_slice(&response).map_err(Into::into)
    }
}

impl Method<Near> for FetchNonceRequest {
    const METHOD: &'static str = "fetch_nonce";

    type Returns = Option<u64>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        serde_json::from_slice(&response).map_err(Into::into)
    }
}
