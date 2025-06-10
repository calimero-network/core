use std::future::Future;

use calimero_context_config::client::env::config::ContextConfig;
use calimero_context_config::repr::{Repr, ReprBytes, ReprTransmute};
use calimero_context_config::types::{self as types, Capability};
use calimero_primitives::application::{Application, ApplicationBlob};
use calimero_primitives::blobs::BlobId;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use eyre::{bail, OptionExt};

use super::ExternalClient;

const MAX_RETRIES: u8 = 3;

#[derive(Debug)]
pub struct ExternalConfigClient<'a> {
    nonce: Option<u64>,
    client: &'a ExternalClient<'a>,
}

impl ExternalClient<'_> {
    pub const fn config(&self) -> ExternalConfigClient<'_> {
        ExternalConfigClient {
            nonce: None,
            client: self,
        }
    }
}

impl ExternalConfigClient<'_> {
    pub async fn fetch_nonce(&self, public_key: &PublicKey) -> eyre::Result<u64> {
        let client = self.client.query::<ContextConfig>(
            self.client.config.protocol.as_ref().into(),
            self.client.config.network_id.as_ref().into(),
            self.client.config.contract_id.as_ref().into(),
        );

        let context_id = self.client.context_id.rt().expect("infallible conversion");
        let public_key = public_key.rt().expect("infallible conversion");

        let nonce = client
            .fetch_nonce(context_id, public_key)
            .await?
            .ok_or_eyre("not a member of the context")?;

        Ok(nonce)
    }

    async fn with_nonce<T, E, F>(
        &mut self,
        public_key: &PublicKey,
        f: impl Fn(u64) -> F,
    ) -> eyre::Result<T>
    where
        E: Into<eyre::Report>,
        F: Future<Output = Result<T, E>>,
    {
        let retries = MAX_RETRIES + (self.nonce.is_none() as u8);

        for _ in 0..(retries + 1) {
            let mut error = None;

            if let Some(nonce) = self.nonce {
                match f(nonce).await {
                    Ok(value) => return Ok(value),
                    Err(err) => error = Some(err),
                }
            }

            let old = self.nonce;

            self.nonce = Some(self.fetch_nonce(public_key).await?);

            if let Some(err) = error {
                if old == self.nonce {
                    return Err(err.into());
                }
            }
        }

        bail!("max retries exceeded");
    }

    pub async fn add_context(
        &self,
        context_secret: &PrivateKey,
        identity: &PublicKey,
        application: &Application,
    ) -> eyre::Result<()> {
        let client = self.client.mutate::<ContextConfig>(
            self.client.config.protocol.as_ref().into(),
            self.client.config.network_id.as_ref().into(),
            self.client.config.contract_id.as_ref().into(),
        );

        client
            .add_context(
                self.client.context_id.rt().expect("infallible conversion"),
                identity.rt().expect("infallible conversion"),
                types::Application::new(
                    application.id.rt().expect("infallible conversion"),
                    application
                        .blob
                        .bytecode
                        .rt()
                        .expect("infallible conversion"),
                    application.size,
                    types::ApplicationSource(application.source.to_string().into()),
                    types::ApplicationMetadata(Repr::new(application.metadata.as_slice().into())),
                ),
            )
            .send(**context_secret, 0)
            .await?;

        Ok(())
    }

    pub async fn update_application(
        &mut self,
        public_key: &PublicKey,
        application: &Application,
    ) -> eyre::Result<()> {
        let identity = self
            .client
            .context_client()
            .get_identity(&self.client.context_id, public_key)?
            .ok_or_eyre("identity not found")?;

        let private_key = identity.private_key()?;

        self.with_nonce(public_key, async |nonce| {
            let client = self.client.mutate::<ContextConfig>(
                self.client.config.protocol.as_ref().into(),
                self.client.config.network_id.as_ref().into(),
                self.client.config.contract_id.as_ref().into(),
            );

            client
                .update_application(
                    self.client.context_id.rt().expect("infallible conversion"),
                    types::Application::new(
                        application.id.rt().expect("infallible conversion"),
                        application
                            .blob
                            .bytecode
                            .rt()
                            .expect("infallible conversion"),
                        application.size,
                        types::ApplicationSource(application.source.to_string().into()),
                        types::ApplicationMetadata(Repr::new(
                            application.metadata.as_slice().into(),
                        )),
                    ),
                )
                .send(**private_key, nonce)
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn add_members(
        &mut self,
        public_key: &PublicKey,
        identities: &[PublicKey],
    ) -> eyre::Result<()> {
        let identity = self
            .client
            .context_client()
            .get_identity(&self.client.context_id, public_key)?
            .ok_or_eyre("identity not found")?;

        let private_key = identity.private_key()?;

        let identities = identities
            .iter()
            .map(|e| e.rt())
            .collect::<Result<Vec<_>, _>>()
            .expect("infallible conversion");

        self.with_nonce(public_key, async |nonce| {
            let client = self.client.mutate::<ContextConfig>(
                self.client.config.protocol.as_ref().into(),
                self.client.config.network_id.as_ref().into(),
                self.client.config.contract_id.as_ref().into(),
            );

            client
                .add_members(
                    self.client.context_id.rt().expect("infallible conversion"),
                    &identities,
                )
                .send(**private_key, nonce)
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn remove_members(
        &mut self,
        public_key: &PublicKey,
        identities: &[PublicKey],
    ) -> eyre::Result<()> {
        let identity = self
            .client
            .context_client()
            .get_identity(&self.client.context_id, public_key)?
            .ok_or_eyre("identity not found")?;

        let private_key = identity.private_key()?;

        let identities = identities
            .iter()
            .map(|e| e.rt())
            .collect::<Result<Vec<_>, _>>()
            .expect("infallible conversion");

        self.with_nonce(public_key, async |nonce| {
            let client = self.client.mutate::<ContextConfig>(
                self.client.config.protocol.as_ref().into(),
                self.client.config.network_id.as_ref().into(),
                self.client.config.contract_id.as_ref().into(),
            );

            client
                .remove_members(
                    self.client.context_id.rt().expect("infallible conversion"),
                    &identities,
                )
                .send(**private_key, nonce)
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn grant(
        &mut self,
        public_key: &PublicKey,
        capabilities: &[(PublicKey, Capability)],
    ) -> eyre::Result<()> {
        let identity = self
            .client
            .context_client()
            .get_identity(&self.client.context_id, public_key)?
            .ok_or_eyre("identity not found")?;

        let private_key = identity.private_key()?;

        let capabilities = capabilities
            .iter()
            .map(|(who, cap)| who.rt().map(|who| (who, *cap)))
            .collect::<Result<Vec<_>, _>>()
            .expect("infallible conversion");

        self.with_nonce(public_key, async |nonce| {
            let client = self.client.mutate::<ContextConfig>(
                self.client.config.protocol.as_ref().into(),
                self.client.config.network_id.as_ref().into(),
                self.client.config.contract_id.as_ref().into(),
            );

            client
                .grant(
                    self.client.context_id.rt().expect("infallible conversion"),
                    &capabilities,
                )
                .send(**private_key, nonce)
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn revoke(
        &mut self,
        public_key: &PublicKey,
        capabilities: &[(PublicKey, Capability)],
    ) -> eyre::Result<()> {
        let identity = self
            .client
            .context_client()
            .get_identity(&self.client.context_id, public_key)?
            .ok_or_eyre("identity not found")?;

        let private_key = identity.private_key()?;

        let capabilities = capabilities
            .iter()
            .map(|(who, cap)| who.rt().map(|who| (who, *cap)))
            .collect::<Result<Vec<_>, _>>()
            .expect("infallible conversion");

        self.with_nonce(public_key, async |nonce| {
            let client = self.client.mutate::<ContextConfig>(
                self.client.config.protocol.as_ref().into(),
                self.client.config.network_id.as_ref().into(),
                self.client.config.contract_id.as_ref().into(),
            );

            client
                .revoke(
                    self.client.context_id.rt().expect("infallible conversion"),
                    &capabilities,
                )
                .send(**private_key, nonce)
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn update_proxy_contract(&mut self, public_key: &PublicKey) -> eyre::Result<()> {
        let identity = self
            .client
            .context_client()
            .get_identity(&self.client.context_id, public_key)?
            .ok_or_eyre("identity not found")?;

        let private_key = identity.private_key()?;

        self.with_nonce(public_key, async |nonce| {
            let client = self.client.mutate::<ContextConfig>(
                self.client.config.protocol.as_ref().into(),
                self.client.config.network_id.as_ref().into(),
                self.client.config.contract_id.as_ref().into(),
            );

            client
                .update_proxy_contract(self.client.context_id.rt().expect("infallible conversion"))
                .send(**private_key, nonce)
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn application(&self) -> eyre::Result<Application> {
        let client = self.client.query::<ContextConfig>(
            self.client.config.protocol.as_ref().into(),
            self.client.config.network_id.as_ref().into(),
            self.client.config.contract_id.as_ref().into(),
        );

        let application = client
            .application(self.client.context_id.rt().expect("infallible conversion"))
            .await?;

        let application = Application::new(
            application.id.as_bytes().into(),
            ApplicationBlob {
                bytecode: application.blob.as_bytes().into(),
                compiled: BlobId::from([0; 32]),
            },
            application.size,
            application.source.0.parse()?,
            application.metadata.0.into_inner().into_owned(),
        );

        Ok(application)
    }

    pub async fn application_revision(&self) -> eyre::Result<u64> {
        let client = self.client.query::<ContextConfig>(
            self.client.config.protocol.as_ref().into(),
            self.client.config.network_id.as_ref().into(),
            self.client.config.contract_id.as_ref().into(),
        );

        let revision = client
            .application_revision(self.client.context_id.rt().expect("infallible conversion"))
            .await?;

        Ok(revision)
    }

    pub async fn members(&self, offset: usize, length: usize) -> eyre::Result<Vec<PublicKey>> {
        let client = self.client.query::<ContextConfig>(
            self.client.config.protocol.as_ref().into(),
            self.client.config.network_id.as_ref().into(),
            self.client.config.contract_id.as_ref().into(),
        );

        let members = client
            .members(
                self.client.context_id.rt().expect("infallible conversion"),
                offset,
                length,
            )
            .await?;

        let members = members
            .into_iter()
            .map(|identity| identity.as_bytes().into())
            .collect();

        Ok(members)
    }

    pub async fn has_member(&self, identity: &PublicKey) -> eyre::Result<bool> {
        let client = self.client.query::<ContextConfig>(
            self.client.config.protocol.as_ref().into(),
            self.client.config.network_id.as_ref().into(),
            self.client.config.contract_id.as_ref().into(),
        );

        let has_member = client
            .has_member(
                self.client.context_id.rt().expect("infallible conversion"),
                identity.rt().expect("infallible conversion"),
            )
            .await?;

        Ok(has_member)
    }

    pub async fn members_revision(&self) -> eyre::Result<u64> {
        let client = self.client.query::<ContextConfig>(
            self.client.config.protocol.as_ref().into(),
            self.client.config.network_id.as_ref().into(),
            self.client.config.contract_id.as_ref().into(),
        );

        let revision = client
            .members_revision(self.client.context_id.rt().expect("infallible conversion"))
            .await?;

        Ok(revision)
    }

    // todo! reintroduce when PublicKey: Ord
    // pub async fn privileges(
    //     &self,
    //     identities: &[PublicKey],
    // ) -> eyre::Result<BTreeMap<PublicKey, Vec<Capability>>> {
    //     let client = self.client.query::<ContextConfig>(
    //         self.client.config.protocol.as_ref().into(),
    //         self.client.config.network_id.as_ref().into(),
    //         self.client.config.contract_id.as_ref().into(),
    //     );

    //     let identities = identities
    //         .iter()
    //         .map(|e| e.rt())
    //         .collect::<Result<Vec<_>, _>>()
    //         .expect("infallible conversion");

    //     let privileges = client
    //         .privileges(
    //             self.client.context_id.rt().expect("infallible conversion"),
    //             &identities,
    //         )
    //         .await?;

    //     let privileges = privileges
    //         .into_iter()
    //         .map(|(identity, capabilities)| (identity.as_bytes().into(), capabilities))
    //         .collect();

    //     Ok(privileges)
    // }

    pub async fn get_proxy_contract(&self) -> eyre::Result<String> {
        let client = self.client.query::<ContextConfig>(
            self.client.config.protocol.as_ref().into(),
            self.client.config.network_id.as_ref().into(),
            self.client.config.contract_id.as_ref().into(),
        );

        let proxy_contract = client
            .get_proxy_contract(self.client.context_id.rt().expect("infallible conversion"))
            .await?;

        Ok(proxy_contract)
    }
}
