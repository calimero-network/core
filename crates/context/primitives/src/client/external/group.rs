use core::future::Future;

use calimero_context_config::client::env::config::ContextConfig;
use calimero_context_config::client::AnyTransport;
use calimero_context_config::repr::ReprTransmute;
use calimero_context_config::types::{self as types};
use calimero_primitives::context::ContextId;
use ed25519_dalek::SigningKey;
use eyre::bail;

use super::ContextClient;

const MAX_RETRIES: u8 = 3;

pub struct ExternalGroupClient<'a> {
    nonce: Option<u64>,
    signing_key: [u8; 32],
    group_id: types::ContextGroupId,
    protocol: String,
    network_id: String,
    contract_id: String,
    client: &'a ContextClient,
}

impl ContextClient {
    pub fn group_client(
        &self,
        group_id: types::ContextGroupId,
        signing_key: [u8; 32],
        protocol: String,
        network_id: String,
        contract_id: String,
    ) -> ExternalGroupClient<'_> {
        ExternalGroupClient {
            nonce: None,
            signing_key,
            group_id,
            protocol,
            network_id,
            contract_id,
            client: self,
        }
    }
}

impl ExternalGroupClient<'_> {
    fn sdk_client(&self) -> &calimero_context_config::client::Client<AnyTransport> {
        &self.client.external_client
    }

    fn signer_id(&self) -> eyre::Result<types::SignerId> {
        let sk = SigningKey::from_bytes(&self.signing_key);
        sk.verifying_key().rt().map_err(Into::into)
    }

    pub async fn fetch_nonce(&self) -> eyre::Result<u64> {
        let query = self.sdk_client().query::<ContextConfig>(
            self.protocol.as_str().into(),
            self.network_id.as_str().into(),
            self.contract_id.as_str().into(),
        );

        let signer_id = self.signer_id()?;

        let nonce = query
            .fetch_group_nonce(self.group_id, signer_id)
            .await?
            .unwrap_or(0);

        Ok(nonce)
    }

    async fn with_nonce<T, E, F>(&mut self, f: impl Fn(u64) -> F) -> eyre::Result<T>
    where
        E: Into<eyre::Report>,
        F: Future<Output = Result<T, E>>,
    {
        let retries = MAX_RETRIES + u8::from(self.nonce.is_none());

        for _ in 0..=retries {
            let mut error = None;

            if let Some(nonce) = self.nonce {
                match f(nonce).await {
                    Ok(value) => return Ok(value),
                    Err(err) => error = Some(err),
                }
            }

            let old = self.nonce;

            self.nonce = Some(self.fetch_nonce().await?);

            if let Some(err) = error {
                if old == self.nonce {
                    return Err(err.into());
                }
            }
        }

        bail!("max retries exceeded");
    }

    pub async fn create_group(
        &mut self,
        app_key: types::AppKey,
        target_application: types::Application<'_>,
    ) -> eyre::Result<()> {
        let client = self.sdk_client().mutate::<ContextConfig>(
            self.protocol.as_str().into(),
            self.network_id.as_str().into(),
            self.contract_id.as_str().into(),
        );

        client
            .create_group(self.group_id, app_key, target_application)
            .send(self.signing_key, 0)
            .await?;

        Ok(())
    }

    pub async fn delete_group(&mut self) -> eyre::Result<()> {
        self.with_nonce(async |nonce| {
            let client = self.sdk_client().mutate::<ContextConfig>(
                self.protocol.as_str().into(),
                self.network_id.as_str().into(),
                self.contract_id.as_str().into(),
            );

            client
                .delete_group(self.group_id)
                .send(self.signing_key, nonce)
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn add_group_members(&mut self, members: &[types::SignerId]) -> eyre::Result<()> {
        self.with_nonce(async |nonce| {
            let client = self.sdk_client().mutate::<ContextConfig>(
                self.protocol.as_str().into(),
                self.network_id.as_str().into(),
                self.contract_id.as_str().into(),
            );

            client
                .add_group_members(self.group_id, members)
                .send(self.signing_key, nonce)
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn remove_group_members(
        &mut self,
        members: &[types::SignerId],
    ) -> eyre::Result<()> {
        self.with_nonce(async |nonce| {
            let client = self.sdk_client().mutate::<ContextConfig>(
                self.protocol.as_str().into(),
                self.network_id.as_str().into(),
                self.contract_id.as_str().into(),
            );

            client
                .remove_group_members(self.group_id, members)
                .send(self.signing_key, nonce)
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn register_context_in_group(
        &mut self,
        context_id: ContextId,
    ) -> eyre::Result<()> {
        let context_id: types::ContextId = context_id.rt()?;

        self.with_nonce(async |nonce| {
            let client = self.sdk_client().mutate::<ContextConfig>(
                self.protocol.as_str().into(),
                self.network_id.as_str().into(),
                self.contract_id.as_str().into(),
            );

            client
                .register_context_in_group(self.group_id, context_id)
                .send(self.signing_key, nonce)
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn unregister_context_from_group(
        &mut self,
        context_id: ContextId,
    ) -> eyre::Result<()> {
        let context_id: types::ContextId = context_id.rt()?;

        self.with_nonce(async |nonce| {
            let client = self.sdk_client().mutate::<ContextConfig>(
                self.protocol.as_str().into(),
                self.network_id.as_str().into(),
                self.contract_id.as_str().into(),
            );

            client
                .unregister_context_from_group(self.group_id, context_id)
                .send(self.signing_key, nonce)
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn set_group_target(
        &mut self,
        target_application: types::Application<'_>,
    ) -> eyre::Result<()> {
        self.with_nonce(async |nonce| {
            let client = self.sdk_client().mutate::<ContextConfig>(
                self.protocol.as_str().into(),
                self.network_id.as_str().into(),
                self.contract_id.as_str().into(),
            );

            client
                .set_group_target(self.group_id, target_application.clone())
                .send(self.signing_key, nonce)
                .await
        })
        .await?;

        Ok(())
    }
}
