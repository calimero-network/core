use calimero_context_client::local_governance::{AckRouter, GroupOp, NamespaceOp};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::Store;
use eyre::Result as EyreResult;

use crate::governance_broadcast::DeliveryReport;

/// Handles the async sign-encrypt-publish logic for governance operations.
/// Separates the networking concerns from the pure store queries in group_store.
pub struct GovernanceSigner<'a> {
    store: &'a Store,
    node_client: &'a calimero_node_primitives::client::NodeClient,
    ack_router: &'a AckRouter,
}

impl<'a> GovernanceSigner<'a> {
    pub fn new(
        store: &'a Store,
        node_client: &'a calimero_node_primitives::client::NodeClient,
        ack_router: &'a AckRouter,
    ) -> Self {
        Self {
            store,
            node_client,
            ack_router,
        }
    }

    /// `Ok(None)` is a deliberate skip — see
    /// [`GroupGovernancePublisher::sign_apply_and_publish`].
    pub async fn publish_group_op(
        &self,
        group_id: &ContextGroupId,
        signer_sk: &PrivateKey,
        op: GroupOp,
    ) -> EyreResult<Option<DeliveryReport>> {
        super::GroupGovernancePublisher::new(self.store, self.node_client, *group_id)
            .sign_apply_and_publish(self.ack_router, signer_sk, op)
            .await
    }

    /// `Ok(None)` is a deliberate skip — see
    /// [`GroupGovernancePublisher::sign_apply_and_publish_removal`].
    pub async fn publish_group_removal(
        &self,
        group_id: &ContextGroupId,
        signer_sk: &PrivateKey,
        removed_member: &PublicKey,
    ) -> EyreResult<Option<DeliveryReport>> {
        super::GroupGovernancePublisher::new(self.store, self.node_client, *group_id)
            .sign_apply_and_publish_removal(self.ack_router, signer_sk, removed_member)
            .await
    }

    pub async fn publish_namespace_op(
        &self,
        namespace_id: [u8; 32],
        signer_sk: &PrivateKey,
        op: NamespaceOp,
    ) -> EyreResult<DeliveryReport> {
        super::NamespaceGovernance::new(self.store, namespace_id)
            .sign_apply_and_publish(self.node_client, self.ack_router, signer_sk, op)
            .await
    }

    pub async fn publish_namespace_op_without_apply(
        &self,
        namespace_id: [u8; 32],
        signer_sk: &PrivateKey,
        op: NamespaceOp,
        required_signers: Option<Vec<PublicKey>>,
    ) -> EyreResult<DeliveryReport> {
        super::NamespaceGovernance::new(self.store, namespace_id)
            .sign_and_publish_without_apply(
                self.node_client,
                self.ack_router,
                signer_sk,
                op,
                required_signers,
            )
            .await
    }
}
