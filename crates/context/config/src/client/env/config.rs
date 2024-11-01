use std::borrow::Cow;
use std::collections::BTreeMap;
use std::ptr;

use mutate::context::Mutate;
use query::application::ApplicationRequest;
use query::application_revision::{ApplicationRevision, Revision};
use query::members::Members;
use query::members_revision::MembersRevision;
use query::privileges::IdentitiyPrivileges;

mod mutate;
mod query;

use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::{CallClient, ConfigError, Environment, Protocol, Transport};
use crate::repr::Repr;
use crate::types::{Application, Capability, ContextId, ContextIdentity, SignerId};
use crate::{ContextRequest, ContextRequestKind, RequestKind};
pub enum ContextConfig {}

pub struct ContextConfigQuery<'a, T> {
    client: CallClient<'a, T>,
}

pub struct ContextConfigMutate<'a, T> {
    client: CallClient<'a, T>,
}

impl<'a, T: 'a> Environment<'a, T> for ContextConfig {
    type Query = ContextConfigQuery<'a, T>;
    type Mutate = ContextConfigMutate<'a, T>;

    fn query(client: CallClient<'a, T>) -> Self::Query {
        match client.protocol {
            Protocol::Near => client.client.query::<ContextConfig>(
                Protocol::Near,
                client.network_id,
                client.contract_id,
            ),
            Protocol::Starknet => client.client.query::<ContextConfig>(
                Protocol::Starknet,
                client.network_id,
                client.contract_id,
            ),
        }
    }

    fn mutate(client: CallClient<'a, T>) -> Self::Mutate {
        match client.protocol {
            Protocol::Near => client.client.mutate::<ContextConfig>(
                Protocol::Near,
                client.network_id,
                client.contract_id,
            ),
            Protocol::Starknet => todo!(),
        }
    }
}

impl<'a, T: Transport> ContextConfigQuery<'a, T> {
    pub async fn members(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<Vec<Repr<ContextIdentity>>, ConfigError<T>> {
        let params = Members { offset, length };
        match self.client.protocol {
            Protocol::Near => self.client.query::<Near, Members>(params).await,
            Protocol::Starknet => self.client.query::<Starknet, Members>(params).await,
        }
    }
    pub async fn application_revision(&self) -> Result<Revision, ConfigError<T>> {
        let params = ApplicationRevision {};
        match self.client.protocol {
            Protocol::Near => self.client.query::<Near, ApplicationRevision>(params).await,
            Protocol::Starknet => {
                self.client
                    .query::<Starknet, ApplicationRevision>(params)
                    .await
            }
        }
    }
    pub async fn application(&self) -> Result<Application<'static>, ConfigError<T>> {
        let params = ApplicationRequest {};
        match self.client.protocol {
            Protocol::Near => self.client.query::<Near, ApplicationRequest>(params).await,
            Protocol::Starknet => {
                self.client
                    .query::<Starknet, ApplicationRequest>(params)
                    .await
            }
        }
    }

    pub async fn members_revision(&self) -> Result<Revision, ConfigError<T>> {
        let params = MembersRevision {};
        match self.client.protocol {
            Protocol::Near => self.client.query::<Near, MembersRevision>(params).await,
            Protocol::Starknet => self.client.query::<Starknet, MembersRevision>(params).await,
        }
    }

    pub async fn privileges(
        &self,
        identities: &[ContextIdentity],
    ) -> Result<BTreeMap<Repr<SignerId>, Vec<Capability>>, ConfigError<T>> {
        let params = IdentitiyPrivileges { identities };
        match self.client.protocol {
            Protocol::Near => {
                self.client
                    .query::<Near, IdentitiyPrivileges<'_>>(params)
                    .await
            }
            Protocol::Starknet => {
                self.client
                    .query::<Starknet, IdentitiyPrivileges<'_>>(params)
                    .await
            }
        }
    }
}

#[derive(Debug)]
pub struct ContextConfigMutateRequest<'a, T> {
    client: CallClient<'a, T>,
    kind: RequestKind<'a>,
}

impl<'a, T: Transport> ContextConfigMutate<'a, T> {
    pub fn add_context(
        self,
        context_id: ContextId,
        author_id: ContextIdentity,
        application: Application<'a>,
    ) -> ContextConfigMutateRequest<'a, T> {
        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Context(ContextRequest {
                context_id: Repr::new(context_id),
                kind: ContextRequestKind::Add {
                    author_id: Repr::new(author_id),
                    application,
                },
            }),
        }
    }

    pub fn update_application(
        self,
        context_id: ContextId,
        application: Application<'a>,
    ) -> ContextConfigMutateRequest<'a, T> {
        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Context(ContextRequest {
                context_id: Repr::new(context_id),
                kind: ContextRequestKind::UpdateApplication { application },
            }),
        }
    }

    pub fn add_members(
        self,
        context_id: ContextId,
        members: &[ContextIdentity],
    ) -> ContextConfigMutateRequest<'a, T> {
        let members = unsafe {
            &*(ptr::from_ref::<[ContextIdentity]>(members) as *const [Repr<ContextIdentity>])
        };

        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Context(ContextRequest {
                context_id: Repr::new(context_id),
                kind: ContextRequestKind::AddMembers {
                    members: Cow::Borrowed(members),
                },
            }),
        }
    }

    pub fn remove_members(
        self,
        context_id: ContextId,
        members: &[ContextIdentity],
    ) -> ContextConfigMutateRequest<'a, T> {
        let members = unsafe {
            &*(ptr::from_ref::<[ContextIdentity]>(members) as *const [Repr<ContextIdentity>])
        };

        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Context(ContextRequest {
                context_id: Repr::new(context_id),
                kind: ContextRequestKind::RemoveMembers {
                    members: Cow::Borrowed(members),
                },
            }),
        }
    }

    pub fn grant(
        self,
        context_id: ContextId,
        capabilities: &[(ContextIdentity, Capability)],
    ) -> ContextConfigMutateRequest<'a, T> {
        let capabilities = unsafe {
            &*(ptr::from_ref::<[(ContextIdentity, Capability)]>(capabilities)
                as *const [(Repr<ContextIdentity>, Capability)])
        };

        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Context(ContextRequest {
                context_id: Repr::new(context_id),
                kind: ContextRequestKind::Grant {
                    capabilities: Cow::Borrowed(capabilities),
                },
            }),
        }
    }

    pub fn revoke(
        self,
        context_id: ContextId,
        capabilities: &[(ContextIdentity, Capability)],
    ) -> ContextConfigMutateRequest<'a, T> {
        let capabilities = unsafe {
            &*(ptr::from_ref::<[(ContextIdentity, Capability)]>(capabilities)
                as *const [(Repr<ContextIdentity>, Capability)])
        };

        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::Context(ContextRequest {
                context_id: Repr::new(context_id),
                kind: ContextRequestKind::Revoke {
                    capabilities: Cow::Borrowed(capabilities),
                },
            }),
        }
    }
}

impl<'a, T: Transport> ContextConfigMutateRequest<'a, T> {
    pub async fn send(self, signing_key: [u8; 32]) -> Result<(), ConfigError<T>> {
        let request = Mutate {
            signer_id: signing_key,
            nonce: 0,
            kind: self.kind,
        };

        match self.client.protocol {
            Protocol::Near => self.client.mutate::<Near, Mutate<'_>>(request).await?,
            Protocol::Starknet => self.client.mutate::<Starknet, Mutate<'_>>(request).await?,
        }

        Ok(())
    }
}
