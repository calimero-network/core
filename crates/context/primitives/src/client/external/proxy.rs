use calimero_context_config::client::env::proxy::ContextProxy;
use calimero_context_config::repr::{ReprBytes, ReprTransmute};
use calimero_context_config::types::{ContextStorageEntry, ProposalId};
use calimero_context_config::{Proposal, ProposalAction};
use calimero_primitives::identity::PublicKey;
use eyre::OptionExt;

use super::ExternalClient;

#[derive(Debug)]
pub struct ExternalProxyClient<'a> {
    client: &'a ExternalClient<'a>,
}

impl ExternalClient<'_> {
    pub fn proxy(&self) -> ExternalProxyClient<'_> {
        ExternalProxyClient { client: self }
    }
}

impl ExternalProxyClient<'_> {
    pub async fn propose(
        &self,
        public_key: &PublicKey,
        proposal_id: &ProposalId,
        actions: Vec<ProposalAction>,
    ) -> eyre::Result<()> {
        let signer_id = public_key.rt().expect("infallible conversion");

        let identity = self
            .client
            .context_client()
            .get_identity(&self.client.context_id, public_key)?
            .ok_or_eyre("identity not found")?;

        let private_key = identity.private_key()?;

        let client = self.client.mutate::<ContextProxy>(
            self.client.config.protocol.as_ref().into(),
            self.client.config.network_id.as_ref().into(),
            self.client.config.proxy_contract.as_ref().into(),
        );

        let _ignored = client
            .propose(*proposal_id, signer_id, actions)
            .send(**private_key)
            .await?;

        Ok(())
    }

    pub async fn approve(
        &self,
        public_key: &PublicKey,
        proposal_id: &ProposalId,
    ) -> eyre::Result<()> {
        let signer_id = public_key.rt().expect("infallible conversion");

        let identity = self
            .client
            .context_client()
            .get_identity(&self.client.context_id, public_key)?
            .ok_or_eyre("identity not found")?;

        let private_key = identity.private_key()?;

        let client = self.client.mutate::<ContextProxy>(
            self.client.config.protocol.as_ref().into(),
            self.client.config.network_id.as_ref().into(),
            self.client.config.proxy_contract.as_ref().into(),
        );

        let _ignored = client
            .approve(signer_id, *proposal_id)
            .send(**private_key)
            .await?;

        Ok(())
    }

    pub async fn active_proposals(&self) -> eyre::Result<u16> {
        let client = self.client.query::<ContextProxy>(
            self.client.config.protocol.as_ref().into(),
            self.client.config.network_id.as_ref().into(),
            self.client.config.proxy_contract.as_ref().into(),
        );

        let proposals = client.get_number_of_active_proposals().await?;

        Ok(proposals)
    }

    pub async fn get_external_proxy_contract(&self) -> eyre::Result<String> {
        Ok(self.client.config.proxy_contract.clone().into_owned())
    }

    pub async fn get_external_value(&self, stored_key: Vec<u8>) -> eyre::Result<Vec<u8>> {
        let client = self.client.query::<ContextProxy>(
            self.client.config.protocol.as_ref().into(),
            self.client.config.network_id.as_ref().into(),
            self.client.config.proxy_contract.as_ref().into(),
        );

        let value = client.get_context_value(stored_key).await?;

        Ok(value)
    }

    pub async fn get_proposal(&self, proposal_id: &ProposalId) -> eyre::Result<Option<Proposal>> {
        let client = self.client.query::<ContextProxy>(
            self.client.config.protocol.as_ref().into(),
            self.client.config.network_id.as_ref().into(),
            self.client.config.proxy_contract.as_ref().into(),
        );

        let proposal = client.proposal(*proposal_id).await?;

        Ok(proposal)
    }

    pub async fn get_proposal_approvers(
        &self,
        proposal_id: &ProposalId,
    ) -> eyre::Result<impl Iterator<Item = PublicKey>> {
        let client = self.client.query::<ContextProxy>(
            self.client.config.protocol.as_ref().into(),
            self.client.config.network_id.as_ref().into(),
            self.client.config.proxy_contract.as_ref().into(),
        );

        let approvers = client.get_proposal_approvers(*proposal_id).await?;

        let approvers = approvers
            .into_iter()
            .map(|identity| identity.as_bytes().into());

        Ok(approvers)
    }

    pub async fn get_proposals(&self, offset: usize, limit: usize) -> eyre::Result<Vec<Proposal>> {
        let client = self.client.query::<ContextProxy>(
            self.client.config.protocol.as_ref().into(),
            self.client.config.network_id.as_ref().into(),
            self.client.config.proxy_contract.as_ref().into(),
        );

        let proposals = client.proposals(offset, limit).await?;

        Ok(proposals)
    }

    pub async fn proposal_approvals(&self, proposal_id: &ProposalId) -> eyre::Result<usize> {
        let client = self.client.query::<ContextProxy>(
            self.client.config.protocol.as_ref().into(),
            self.client.config.network_id.as_ref().into(),
            self.client.config.proxy_contract.as_ref().into(),
        );

        let approvals = client
            .get_number_of_proposal_approvals(*proposal_id)
            .await?;

        Ok(approvals.num_approvals)
    }

    pub async fn get_external_storage_entries(
        &self,
        offset: usize,
        limit: usize,
    ) -> eyre::Result<Vec<ContextStorageEntry>> {
        let client = self.client.query::<ContextProxy>(
            self.client.config.protocol.as_ref().into(),
            self.client.config.network_id.as_ref().into(),
            self.client.config.proxy_contract.as_ref().into(),
        );

        let entries = client.get_context_storage_entries(offset, limit).await?;

        Ok(entries)
    }
}
