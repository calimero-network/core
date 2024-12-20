use std::collections::BTreeMap;

use crate::client::env::utils;
use crate::client::transport::Transport;
use crate::client::{CallClient, ClientError, Operation};
use crate::repr::Repr;
use crate::types::{Application, Capability, ContextId, ContextIdentity, Revision, SignerId};

pub mod application;
pub mod application_revision;
pub mod fetch_nonce;
pub mod has_member;
pub mod members;
pub mod members_revision;
pub mod privileges;
pub mod proxy_contract;

#[derive(Debug)]
pub struct ContextConfigQuery<'a, T> {
    pub client: CallClient<'a, T>,
}

impl<'a, T: Transport> ContextConfigQuery<'a, T> {
    pub async fn application(
        &self,
        context_id: ContextId,
    ) -> Result<Application<'static>, ClientError<T>> {
        let params = application::ApplicationRequest {
            context_id: Repr::new(context_id),
        };

        utils::send(&self.client, Operation::Read(params)).await
    }

    pub async fn application_revision(
        &self,
        context_id: ContextId,
    ) -> Result<Revision, ClientError<T>> {
        let params = application_revision::ApplicationRevisionRequest {
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
        let params = members::MembersRequest {
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
        let params = has_member::HasMemberRequest {
            context_id: Repr::new(context_id),
            identity: Repr::new(identity),
        };

        utils::send(&self.client, Operation::Read(params)).await
    }

    pub async fn members_revision(
        &self,
        context_id: ContextId,
    ) -> Result<Revision, ClientError<T>> {
        let params = members_revision::MembersRevisionRequest {
            context_id: Repr::new(context_id),
        };

        utils::send(&self.client, Operation::Read(params)).await
    }

    pub async fn privileges(
        &self,
        context_id: ContextId,
        identities: &[ContextIdentity],
    ) -> Result<BTreeMap<SignerId, Vec<Capability>>, ClientError<T>> {
        let params = privileges::PrivilegesRequest::new(context_id, identities);

        utils::send(&self.client, Operation::Read(params)).await
    }

    pub async fn get_proxy_contract(
        &self,
        context_id: ContextId,
    ) -> Result<String, ClientError<T>> {
        let params = proxy_contract::ProxyContractRequest {
            context_id: Repr::new(context_id),
        };

        utils::send(&self.client, Operation::Read(params)).await
    }

    pub async fn fetch_nonce(
        &self,
        context_id: ContextId,
        member_id: ContextIdentity,
    ) -> Result<Option<u64>, ClientError<T>> {
        let params = fetch_nonce::FetchNonceRequest::new(context_id, member_id);

        utils::send(&self.client, Operation::Read(params)).await
    }
}
