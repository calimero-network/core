//! Store-backed context config access. On-chain “config” and “proxy” clients are removed;
//! mutating methods are no-ops kept for call-site compatibility.

use calimero_context_config::types::{BlockHeight, Capability, SignedRevealPayload};
use calimero_primitives::application::Application;
use calimero_primitives::context::{ContextConfigParams, ContextId};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::key;
use eyre::OptionExt;
use futures_util::pin_mut;
use futures_util::StreamExt;

use super::ContextClient;

impl ContextClient {
    pub fn context_config(
        &self,
        context_id: &ContextId,
    ) -> eyre::Result<Option<ContextConfigParams<'static>>> {
        let handle = self.datastore.handle();

        let key = key::ContextConfig::new(*context_id);

        let Some(config) = handle.get(&key)? else {
            return Ok(None);
        };

        let context_config = ContextConfigParams {
            protocol: config.protocol.into_string().into(),
            network_id: config.network.into_string().into(),
            contract_id: config.contract.into_string().into(),
            proxy_contract: config.proxy_contract.into_string().into(),
            application_revision: config.application_revision,
            members_revision: config.members_revision,
        };

        Ok(Some(context_config))
    }

    pub async fn get_context_application(&self, context_id: &ContextId) -> eyre::Result<Application> {
        let handle = self.datastore.handle();
        let meta = handle
            .get(&key::ContextMeta::new(*context_id))?
            .ok_or_eyre("context meta not found")?;
        let app_id = meta.application.application_id();
        self.node_client()
            .get_application(&app_id)?
            .ok_or_eyre("application not found")
    }

    pub async fn get_context_application_revision(
        &self,
        context_id: &ContextId,
    ) -> eyre::Result<u64> {
        let handle = self.datastore.handle();
        let cfg = handle
            .get(&key::ContextConfig::new(*context_id))?
            .ok_or_eyre("context config not found")?;
        Ok(cfg.application_revision)
    }

    pub async fn get_context_member_page(
        &self,
        context_id: &ContextId,
        offset: usize,
        length: usize,
    ) -> eyre::Result<Vec<PublicKey>> {
        let mut out = Vec::new();
        let stream = self.get_context_members(context_id, None);
        pin_mut!(stream);
        let mut skip = offset;
        while out.len() < length {
            let Some(next) = stream.next().await else {
                break;
            };
            let (pk, _) = next?;
            if skip > 0 {
                skip -= 1;
                continue;
            }
            out.push(pk);
        }
        Ok(out)
    }

    pub async fn get_context_members_revision(&self, context_id: &ContextId) -> eyre::Result<u64> {
        let handle = self.datastore.handle();
        let cfg = handle
            .get(&key::ContextConfig::new(*context_id))?
            .ok_or_eyre("context config not found")?;
        Ok(cfg.members_revision)
    }

    // --- No-op config mutators (local-only; chain removed) ---

    pub async fn noop_config_add_context(
        &self,
        _context_secret: &PrivateKey,
        _identity: &PublicKey,
        _application: &Application,
    ) -> eyre::Result<()> {
        Ok(())
    }

    pub async fn noop_config_update_application(
        &self,
        _public_key: &PublicKey,
        _application: &Application,
    ) -> eyre::Result<()> {
        Ok(())
    }

    pub async fn noop_join_context_commit_invitation(
        &self,
        _public_key: &PublicKey,
        _commitment_hash: String,
        _expiration_block_height: BlockHeight,
    ) -> eyre::Result<()> {
        Ok(())
    }

    pub async fn noop_join_context_reveal_invitation(
        &self,
        _public_key: &PublicKey,
        _payload: SignedRevealPayload,
    ) -> eyre::Result<()> {
        Ok(())
    }

    pub async fn noop_config_add_members(
        &self,
        _public_key: &PublicKey,
        _identities: &[PublicKey],
    ) -> eyre::Result<()> {
        Ok(())
    }

    pub async fn noop_config_remove_members(
        &self,
        _public_key: &PublicKey,
        _identities: &[PublicKey],
    ) -> eyre::Result<()> {
        Ok(())
    }

    pub async fn noop_config_grant(
        &self,
        _public_key: &PublicKey,
        _capabilities: &[(PublicKey, Capability)],
    ) -> eyre::Result<()> {
        Ok(())
    }

    pub async fn noop_config_revoke(
        &self,
        _public_key: &PublicKey,
        _capabilities: &[(PublicKey, Capability)],
    ) -> eyre::Result<()> {
        Ok(())
    }
}
