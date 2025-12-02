use std::collections::BTreeMap;

use borsh::BorshSerialize;

use crate::client::env::config::requests::{
    ApplicationRequest, ApplicationRevisionRequest, FetchNonceRequest, HasMemberRequest,
    MembersRequest, MembersRevisionRequest, PrivilegesRequest, ProxyContractRequest,
};
use crate::client::env::Method;
use crate::client::protocol::mock_relayer::MockRelayer;
use crate::repr::Repr;
use crate::types::{Application, Capability, ContextId, ContextIdentity, Revision, SignerId};

impl Method<MockRelayer> for ApplicationRequest {
    const METHOD: &'static str = "application";

    type Returns = Application<'static>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        #[derive(BorshSerialize)]
        struct Request {
            context_id: Repr<ContextId>,
        }

        let req = Request {
            context_id: self.context_id,
        };

        borsh::to_vec(&req).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        borsh::from_slice(&response).map_err(Into::into)
    }
}

impl Method<MockRelayer> for ApplicationRevisionRequest {
    const METHOD: &'static str = "application_revision";

    type Returns = Revision;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        #[derive(BorshSerialize)]
        struct Request {
            context_id: Repr<ContextId>,
        }

        let req = Request {
            context_id: self.context_id,
        };

        borsh::to_vec(&req).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        borsh::from_slice(&response).map_err(Into::into)
    }
}

impl Method<MockRelayer> for MembersRequest {
    const METHOD: &'static str = "members";

    type Returns = Vec<ContextIdentity>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        #[derive(BorshSerialize)]
        struct Request {
            context_id: Repr<ContextId>,
            offset: usize,
            length: usize,
        }

        let req = Request {
            context_id: self.context_id,
            offset: self.offset,
            length: self.length,
        };

        borsh::to_vec(&req).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        borsh::from_slice(&response).map_err(Into::into)
    }
}

impl Method<MockRelayer> for HasMemberRequest {
    const METHOD: &'static str = "has_member";

    type Returns = bool;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        #[derive(BorshSerialize)]
        struct Request {
            context_id: Repr<ContextId>,
            identity: Repr<ContextIdentity>,
        }

        let req = Request {
            context_id: self.context_id,
            identity: self.identity,
        };

        borsh::to_vec(&req).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        borsh::from_slice(&response).map_err(Into::into)
    }
}

impl Method<MockRelayer> for MembersRevisionRequest {
    const METHOD: &'static str = "members_revision";

    type Returns = Revision;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        #[derive(BorshSerialize)]
        struct Request {
            context_id: Repr<ContextId>,
        }

        let req = Request {
            context_id: self.context_id,
        };

        borsh::to_vec(&req).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        borsh::from_slice(&response).map_err(Into::into)
    }
}

impl<'a> Method<MockRelayer> for PrivilegesRequest<'a> {
    const METHOD: &'static str = "privileges";

    type Returns = BTreeMap<SignerId, Vec<Capability>>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        #[derive(BorshSerialize)]
        struct Request {
            context_id: Repr<ContextId>,
            identities: Vec<Repr<ContextIdentity>>,
        }

        let req = Request {
            context_id: self.context_id,
            identities: self.identities.to_vec(),
        };

        borsh::to_vec(&req).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        // Mock relayer does not currently serialize privilege data, so treat any response as empty.
        let _ = response;
        Ok(BTreeMap::new())
    }
}

impl Method<MockRelayer> for ProxyContractRequest {
    const METHOD: &'static str = "get_proxy_contract";

    type Returns = String;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        #[derive(BorshSerialize)]
        struct Request {
            context_id: Repr<ContextId>,
        }

        let req = Request {
            context_id: self.context_id,
        };

        borsh::to_vec(&req).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        borsh::from_slice(&response).map_err(Into::into)
    }
}

impl Method<MockRelayer> for FetchNonceRequest {
    const METHOD: &'static str = "fetch_nonce";

    type Returns = Option<u64>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        #[derive(BorshSerialize)]
        struct Request {
            context_id: Repr<ContextId>,
            member_id: Repr<ContextIdentity>,
        }

        let req = Request {
            context_id: self.context_id,
            member_id: self.member_id,
        };

        borsh::to_vec(&req).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        borsh::from_slice(&response).map_err(Into::into)
    }
}
