use calimero_context_client::local_governance::{GroupOp, NamespaceOp};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::Store;
use eyre::Result as EyreResult;
use rand::{rngs::OsRng, Rng};

use super::{
    build_key_rotation, encrypt_group_op, get_namespace_identity_record,
    load_current_group_key_record, resolve_namespace, sign_apply_local_group_op_borsh,
    store_group_key, NamespaceGovernance,
};

/// Orchestrates local apply + encrypted namespace publish for group governance ops.
pub struct GroupGovernancePublisher<'a> {
    store: &'a Store,
    node_client: &'a calimero_node_primitives::client::NodeClient,
    group_id: ContextGroupId,
}

impl<'a> GroupGovernancePublisher<'a> {
    pub fn new(
        store: &'a Store,
        node_client: &'a calimero_node_primitives::client::NodeClient,
        group_id: ContextGroupId,
    ) -> Self {
        Self {
            store,
            node_client,
            group_id,
        }
    }

    pub async fn sign_apply_and_publish(
        &self,
        signer_sk: &PrivateKey,
        op: GroupOp,
    ) -> EyreResult<()> {
        self.sign_apply_and_publish_inner(signer_sk, op, None).await
    }

    pub async fn sign_apply_and_publish_removal(
        &self,
        signer_sk: &PrivateKey,
        removed_member: &PublicKey,
    ) -> EyreResult<()> {
        self.sign_apply_and_publish_inner(
            signer_sk,
            GroupOp::MemberRemoved {
                member: *removed_member,
            },
            Some(removed_member),
        )
        .await
    }

    async fn sign_apply_and_publish_inner(
        &self,
        signer_sk: &PrivateKey,
        op: GroupOp,
        removed_member: Option<&PublicKey>,
    ) -> EyreResult<()> {
        let _output =
            sign_apply_local_group_op_borsh(self.store, &self.group_id, signer_sk, op.clone())?;

        let namespace_id = resolve_namespace(self.store, &self.group_id)?;
        let namespace_bytes = namespace_id.to_bytes();

        let Some(namespace_identity) = get_namespace_identity_record(self.store, &namespace_id)?
        else {
            tracing::debug!(
                group_id = %hex::encode(self.group_id.to_bytes()),
                "no namespace identity, skipping namespace publish"
            );
            return Ok(());
        };

        let Some(stored_key) = load_current_group_key_record(self.store, &self.group_id)? else {
            tracing::debug!(
                group_id = %hex::encode(self.group_id.to_bytes()),
                "no group key stored, skipping namespace publish"
            );
            return Ok(());
        };

        let encrypted = encrypt_group_op(&stored_key.group_key, &op)?;

        let key_rotation = if let Some(removed) = removed_member {
            let new_group_key: [u8; 32] = OsRng.gen();
            let _ = store_group_key(self.store, &self.group_id, &new_group_key)?;
            Some(build_key_rotation(
                self.store,
                &self.group_id,
                &new_group_key,
                signer_sk,
                Some(removed),
            )?)
        } else {
            None
        };

        let namespace_op = NamespaceOp::Group {
            group_id: self.group_id.to_bytes(),
            key_id: stored_key.key_id,
            encrypted,
            key_rotation,
        };

        let namespace_sk = PrivateKey::from(namespace_identity.private_key);
        NamespaceGovernance::new(self.store, namespace_bytes)
            .sign_and_publish_without_apply(self.node_client, &namespace_sk, namespace_op)
            .await
    }
}
