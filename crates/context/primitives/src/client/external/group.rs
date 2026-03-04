use core::future::Future;

use calimero_context_config::client::env::config::ContextConfig;
use calimero_context_config::client::{AnyTransport, Client};
use calimero_context_config::repr::ReprTransmute;
use calimero_context_config::types::{self as types};
use calimero_primitives::context::ContextId;
use ed25519_dalek::SigningKey;
use eyre::bail;

use super::ContextClient;

const MAX_RETRIES: u8 = 3;

/// Immutable configuration for a group contract client.
///
/// Separated from the mutable `nonce` field so that Rust 2021's field-level
/// closure capture allows closures to borrow `inner` while `nonce` is
/// mutably borrowed by the retry loop.
struct GroupClientInner {
    signing_key: [u8; 32],
    group_id: types::ContextGroupId,
    protocol: String,
    network_id: String,
    contract_id: String,
    sdk_client: Client<AnyTransport>,
}

/// A contract client for group operations.
///
/// Unlike `ExternalConfigClient` (context-scoped, borrows `ContextClient`),
/// this struct owns a clone of the SDK `Client<AnyTransport>` so it can be
/// moved into `'static` async blocks inside actix handlers.
pub struct ExternalGroupClient {
    nonce: Option<u64>,
    inner: GroupClientInner,
}

impl ContextClient {
    pub fn group_client(
        &self,
        group_id: types::ContextGroupId,
        signing_key: [u8; 32],
        protocol: String,
        network_id: String,
        contract_id: String,
    ) -> ExternalGroupClient {
        ExternalGroupClient {
            nonce: None,
            inner: GroupClientInner {
                signing_key,
                group_id,
                protocol,
                network_id,
                contract_id,
                sdk_client: self.external_client.clone(),
            },
        }
    }
}

impl GroupClientInner {
    fn signer_id(&self) -> eyre::Result<types::SignerId> {
        let sk = SigningKey::from_bytes(&self.signing_key);
        sk.verifying_key().rt().map_err(Into::into)
    }

    async fn fetch_nonce(&self) -> eyre::Result<u64> {
        let query = self.sdk_client.query::<ContextConfig>(
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
}

/// Retry loop with nonce management.
///
/// Free function (not a method) so the call site can split borrows:
/// `&mut self.nonce` is mutably borrowed while `&self.inner` is shared.
async fn with_nonce<T, E, F>(
    nonce: &mut Option<u64>,
    inner: &GroupClientInner,
    f: impl Fn(u64) -> F,
) -> eyre::Result<T>
where
    E: Into<eyre::Report>,
    F: Future<Output = Result<T, E>>,
{
    let retries = MAX_RETRIES + u8::from(nonce.is_none());

    for _ in 0..=retries {
        let mut error = None;

        if let Some(n) = *nonce {
            match f(n).await {
                Ok(value) => return Ok(value),
                Err(err) => error = Some(err),
            }
        }

        let old = *nonce;

        *nonce = Some(inner.fetch_nonce().await?);

        if let Some(err) = error {
            if old == *nonce {
                return Err(err.into());
            }
        }
    }

    bail!("max retries exceeded");
}

impl ExternalGroupClient {
    pub async fn create_group(
        &mut self,
        app_key: types::AppKey,
        target_application: types::Application<'_>,
    ) -> eyre::Result<()> {
        let c = &self.inner;

        c.sdk_client
            .mutate::<ContextConfig>(
                c.protocol.as_str().into(),
                c.network_id.as_str().into(),
                c.contract_id.as_str().into(),
            )
            .create_group(c.group_id, app_key, target_application)
            .send(c.signing_key, 0)
            .await?;

        Ok(())
    }

    pub async fn delete_group(&mut self) -> eyre::Result<()> {
        with_nonce(&mut self.nonce, &self.inner, async |nonce| {
            let c = &self.inner;
            c.sdk_client
                .mutate::<ContextConfig>(
                    c.protocol.as_str().into(),
                    c.network_id.as_str().into(),
                    c.contract_id.as_str().into(),
                )
                .delete_group(c.group_id)
                .send(c.signing_key, nonce)
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn add_group_members(&mut self, members: &[types::SignerId]) -> eyre::Result<()> {
        with_nonce(&mut self.nonce, &self.inner, async |nonce| {
            let c = &self.inner;
            c.sdk_client
                .mutate::<ContextConfig>(
                    c.protocol.as_str().into(),
                    c.network_id.as_str().into(),
                    c.contract_id.as_str().into(),
                )
                .add_group_members(c.group_id, members)
                .send(c.signing_key, nonce)
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn remove_group_members(
        &mut self,
        members: &[types::SignerId],
    ) -> eyre::Result<()> {
        with_nonce(&mut self.nonce, &self.inner, async |nonce| {
            let c = &self.inner;
            c.sdk_client
                .mutate::<ContextConfig>(
                    c.protocol.as_str().into(),
                    c.network_id.as_str().into(),
                    c.contract_id.as_str().into(),
                )
                .remove_group_members(c.group_id, members)
                .send(c.signing_key, nonce)
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

        with_nonce(&mut self.nonce, &self.inner, async |nonce| {
            let c = &self.inner;
            c.sdk_client
                .mutate::<ContextConfig>(
                    c.protocol.as_str().into(),
                    c.network_id.as_str().into(),
                    c.contract_id.as_str().into(),
                )
                .register_context_in_group(c.group_id, context_id)
                .send(c.signing_key, nonce)
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

        with_nonce(&mut self.nonce, &self.inner, async |nonce| {
            let c = &self.inner;
            c.sdk_client
                .mutate::<ContextConfig>(
                    c.protocol.as_str().into(),
                    c.network_id.as_str().into(),
                    c.contract_id.as_str().into(),
                )
                .unregister_context_from_group(c.group_id, context_id)
                .send(c.signing_key, nonce)
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn set_group_target(
        &mut self,
        target_application: types::Application<'_>,
    ) -> eyre::Result<()> {
        with_nonce(&mut self.nonce, &self.inner, async |nonce| {
            let c = &self.inner;
            c.sdk_client
                .mutate::<ContextConfig>(
                    c.protocol.as_str().into(),
                    c.network_id.as_str().into(),
                    c.contract_id.as_str().into(),
                )
                .set_group_target(c.group_id, target_application.clone())
                .send(c.signing_key, nonce)
                .await
        })
        .await?;

        Ok(())
    }
}
