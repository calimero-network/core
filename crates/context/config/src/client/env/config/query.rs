#![expect(clippy::unwrap_in_result, reason = "Repr transmute")]
use std::collections::BTreeMap;

use super::requests::{
    ApplicationRequest, ApplicationRevisionRequest, FetchNonceRequest, HasMemberRequest,
    MembersRequest, MembersRevisionRequest, PrivilegesRequest, ProxyContractRequest,
};
use crate::client::env::utils;
use crate::client::transport::Transport;
use crate::client::{CallClient, ClientError, Operation};
use crate::repr::Repr;
use crate::types::{Application, Capability, ContextId, ContextIdentity, Revision, SignerId};

#[derive(Debug)]
pub struct ContextConfigQuery<'a, T> {
    pub client: CallClient<'a, T>,
}

impl<'a, T: Transport> ContextConfigQuery<'a, T> {
    pub async fn application(
        &self,
        context_id: ContextId,
    ) -> Result<Application<'static>, ClientError<T>> {
        let params = ApplicationRequest {
            context_id: Repr::new(context_id),
        };

        utils::send(&self.client, Operation::Read(params)).await
    }

    pub async fn application_revision(
        &self,
        context_id: ContextId,
    ) -> Result<Revision, ClientError<T>> {
        let params = ApplicationRevisionRequest {
            context_id: Repr::new(context_id),
        };

        utils::send(&self.client, Operation::Read(params)).await
    }

    pub async fn members(
        &self,
        context_id: ContextId,
        offset: usize,
        length: usize,
    ) -> Result<Vec<ContextIdentity>, ClientError<T>> {
        let params = MembersRequest {
            context_id: Repr::new(context_id),
            offset,
            length,
        };

        utils::send(&self.client, Operation::Read(params)).await
    }

    pub async fn has_member(
        &self,
        context_id: ContextId,
        identity: ContextIdentity,
    ) -> Result<bool, ClientError<T>> {
        let params = HasMemberRequest {
            context_id: Repr::new(context_id),
            identity: Repr::new(identity),
        };

        utils::send(&self.client, Operation::Read(params)).await
    }

    pub async fn members_revision(
        &self,
        context_id: ContextId,
    ) -> Result<Revision, ClientError<T>> {
        let params = MembersRevisionRequest {
            context_id: Repr::new(context_id),
        };

        utils::send(&self.client, Operation::Read(params)).await
    }

    pub async fn privileges(
        &self,
        context_id: ContextId,
        identities: &[ContextIdentity],
    ) -> Result<BTreeMap<SignerId, Vec<Capability>>, ClientError<T>> {
        let params = PrivilegesRequest::new(context_id, identities);

        utils::send(&self.client, Operation::Read(params)).await
    }

    pub async fn get_proxy_contract(
        &self,
        context_id: ContextId,
    ) -> Result<String, ClientError<T>> {
        let params = ProxyContractRequest {
            context_id: Repr::new(context_id),
        };

        utils::send(&self.client, Operation::Read(params)).await
    }

    pub async fn fetch_nonce(
        &self,
        context_id: ContextId,
        member_id: ContextIdentity,
    ) -> Result<Option<u64>, ClientError<T>> {
        let params = FetchNonceRequest::new(context_id, member_id);

        utils::send(&self.client, Operation::Read(params)).await
    }
}

// Protocol-specific implementations
pub mod ethereum;
pub mod icp;
pub mod near;
pub mod starknet;
