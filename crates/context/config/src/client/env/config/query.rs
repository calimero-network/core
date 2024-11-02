use std::collections::BTreeMap;

use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::{CallClient, Error, Protocol, Transport};
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

        match self.client.protocol {
            Protocol::Near => self.client.query::<Near, _>(params).await,
            Protocol::Starknet => self.client.query::<Starknet, _>(params).await,
        }
    }

    pub async fn application_revision(&self, context_id: ContextId) -> Result<Revision, Error<T>> {
        let params = application_revision::ApplicationRevisionRequest {
            context_id: Repr::new(context_id),
        };

        match self.client.protocol {
            Protocol::Near => self.client.query::<Near, _>(params).await,
            Protocol::Starknet => self.client.query::<Starknet, _>(params).await,
        }
    }

    pub async fn application(
        &self,
        context_id: ContextId,
    ) -> Result<Application<'static>, Error<T>> {
        let params = application::ApplicationRequest {
            context_id: Repr::new(context_id),
        };

        match self.client.protocol {
            Protocol::Near => self.client.query::<Near, _>(params).await,
            Protocol::Starknet => self.client.query::<Starknet, _>(params).await,
        }
    }

    pub async fn members_revision(&self, context_id: ContextId) -> Result<Revision, Error<T>> {
        let params = members_revision::MembersRevisionRequest {
            context_id: Repr::new(context_id),
        };

        match self.client.protocol {
            Protocol::Near => self.client.query::<Near, _>(params).await,
            Protocol::Starknet => self.client.query::<Starknet, _>(params).await,
        }
    }

    pub async fn privileges(
        &self,
        context_id: ContextId,
        identities: &[ContextIdentity],
    ) -> Result<BTreeMap<SignerId, Vec<Capability>>, Error<T>> {
        let params = privileges::PrivilegesRequest::new(context_id, identities);

        match self.client.protocol {
            Protocol::Near => self.client.query::<Near, _>(params).await,
            Protocol::Starknet => self.client.query::<Starknet, _>(params).await,
        }
    }
}
