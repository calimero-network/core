use std::collections::BTreeMap;

use crate::client::env::utils;
use crate::client::transport::Transport;
use crate::client::{CallClient, Error, Operation};
use crate::repr::Repr;
use crate::types::{Application, Capability, ContextId, ContextIdentity, SignerId};

pub mod application;
pub mod application_revision;
pub mod members;
pub mod members_revision;
pub mod privileges;

// todo! use calimero_context_config::types::Revision when rebased
type Revision = u64;

#[derive(Debug)]
pub struct ContextConfigQuery<'a, T> {
    pub client: CallClient<'a, T>,
}

impl<'a, T: Transport> ContextConfigQuery<'a, T> {
    pub async fn members(
        &self,
        context_id: ContextId,
        offset: usize,
        length: usize,
    ) -> Result<Vec<ContextIdentity>, Error<T>> {
        let params = members::MembersRequest {
            context_id: Repr::new(context_id),
            offset,
            length,
        };

        utils::send_near_or_starknet(&self.client, Operation::Read(params)).await
    }

    pub async fn application_revision(&self, context_id: ContextId) -> Result<Revision, Error<T>> {
        let params = application_revision::ApplicationRevisionRequest {
            context_id: Repr::new(context_id),
        };

        utils::send_near_or_starknet(&self.client, Operation::Read(params)).await
    }

    pub async fn application(
        &self,
        context_id: ContextId,
    ) -> Result<Application<'static>, Error<T>> {
        let params = application::ApplicationRequest {
            context_id: Repr::new(context_id),
        };

        utils::send_near_or_starknet(&self.client, Operation::Read(params)).await
    }

    pub async fn members_revision(&self, context_id: ContextId) -> Result<Revision, Error<T>> {
        let params = members_revision::MembersRevisionRequest {
            context_id: Repr::new(context_id),
        };

        utils::send_near_or_starknet(&self.client, Operation::Read(params)).await
    }

    pub async fn privileges(
        &self,
        context_id: ContextId,
        identities: &[ContextIdentity],
    ) -> Result<BTreeMap<SignerId, Vec<Capability>>, Error<T>> {
        let params = privileges::PrivilegesRequest::new(context_id, identities);

        utils::send_near_or_starknet(&self.client, Operation::Read(params)).await
    }
}
