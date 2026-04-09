use calimero_context_client::local_governance::{
    EncryptedGroupOp, GroupOp, NamespaceOp, RootOp, SignedGroupOp, SignedNamespaceOp,
};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::application::ZERO_APPLICATION_ID;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use crate::metrics::record_namespace_retry_event;

use super::{
    apply_group_op_mutations, count_group_contexts, decrypt_group_op, get_local_gov_nonce,
    get_namespace_identity_record, is_group_admin, load_current_group_key_record,
    load_group_key_by_id, load_group_meta,
    namespace_dag::{NamespaceDagService, NamespaceHead},
    namespace_membership::NamespaceMembershipService,
    namespace_retry::NamespaceRetryService,
    nest_group, save_group_meta, set_local_gov_nonce, store_group_key, unnest_group,
    unwrap_group_key,
};

/// Side effect returned by namespace-op application when an existing
/// member should deliver the group key to a joiner.
#[derive(Debug)]
pub struct PendingKeyDelivery {
    pub namespace_id: [u8; 32],
    pub group_id: [u8; 32],
    pub joiner_pk: PublicKey,
}

/// A key delivery or rotation unwrap failure that the caller should handle.
#[derive(Debug)]
pub struct KeyUnwrapFailure {
    pub group_id: [u8; 32],
    pub reason: String,
}

/// Result of applying a namespace governance op.
#[derive(Debug, Default)]
pub struct ApplyNamespaceOpResult {
    pub pending_deliveries: Vec<PendingKeyDelivery>,
    pub key_unwrap_failures: Vec<KeyUnwrapFailure>,
}

/// Domain API for namespace DAG and governance operation lifecycle.
pub struct NamespaceGovernance<'a> {
    store: &'a Store,
    namespace_id: [u8; 32],
}

impl<'a> NamespaceGovernance<'a> {
    pub fn new(store: &'a Store, namespace_id: [u8; 32]) -> Self {
        Self {
            store,
            namespace_id,
        }
    }

    /// Returns current DAG head as parent hashes + next nonce.
    pub fn read_head_record(&self) -> EyreResult<NamespaceHead> {
        NamespaceDagService::new(self.store, self.namespace_id).read_head_record()
    }

    /// Backward-compatible tuple facade for existing call sites.
    pub fn read_head(&self) -> EyreResult<(Vec<[u8; 32]>, u64)> {
        Ok(self.read_head_record()?.into_tuple())
    }

    pub fn advance_dag_head(
        &self,
        delta_id: [u8; 32],
        parent_ids: &[[u8; 32]],
        sequence: u64,
    ) -> EyreResult<()> {
        NamespaceDagService::new(self.store, self.namespace_id)
            .advance_dag_head(delta_id, parent_ids, sequence)
    }

    /// Persist a namespace governance op in the local DAG log.
    pub fn store_operation(&self, op: &SignedNamespaceOp) -> EyreResult<()> {
        NamespaceDagService::new(self.store, self.namespace_id).store_operation(op)
    }

    pub fn collect_skeleton_delta_ids_for_group(
        &self,
        group_id: [u8; 32],
    ) -> EyreResult<Vec<[u8; 32]>> {
        NamespaceDagService::new(self.store, self.namespace_id)
            .collect_skeleton_delta_ids_for_group(group_id)
    }

    pub fn apply_signed_op(&self, op: &SignedNamespaceOp) -> EyreResult<ApplyNamespaceOpResult> {
        op.verify_signature()
            .map_err(|e| eyre::eyre!("signed namespace op: {e}"))?;

        let mut result = ApplyNamespaceOpResult::default();

        match &op.op {
            NamespaceOp::Root(root) => {
                self.apply_root_op(op, root)?;

                match root {
                    RootOp::KeyDelivery {
                        group_id,
                        ref envelope,
                    } => {
                        let ns_id = ContextGroupId::from(op.namespace_id);
                        if let Some(identity) = get_namespace_identity_record(self.store, &ns_id)? {
                            let recipient_sk = PrivateKey::from(identity.private_key);
                            if envelope.recipient == recipient_sk.public_key() {
                                match unwrap_group_key(&recipient_sk, envelope) {
                                    Ok(group_key) => {
                                        let gid = ContextGroupId::from(*group_id);
                                        let key_id = store_group_key(self.store, &gid, &group_key)?;
                                        tracing::info!(
                                            group_id = %hex::encode(group_id),
                                            key_id = %hex::encode(key_id),
                                            "received group key via KeyDelivery"
                                        );
                                        self.retry_encrypted_ops_for_group(*group_id)?;
                                    }
                                    Err(e) => {
                                        tracing::warn!(?e, "failed to unwrap KeyDelivery envelope");
                                        result.key_unwrap_failures.push(KeyUnwrapFailure {
                                            group_id: *group_id,
                                            reason: format!("KeyDelivery unwrap failed: {e}"),
                                        });
                                    }
                                }
                            }
                        }
                    }
                    RootOp::MemberJoined {
                        member,
                        ref signed_invitation,
                    } => {
                        let gid = signed_invitation.invitation.group_id;
                        let group_id_typed = ContextGroupId::from(gid);
                        if load_current_group_key_record(self.store, &group_id_typed)?.is_some() {
                            result.pending_deliveries.push(PendingKeyDelivery {
                                namespace_id: op.namespace_id,
                                group_id: group_id_typed.to_bytes(),
                                joiner_pk: *member,
                            });
                        }
                    }
                    _ => {}
                }
            }
            NamespaceOp::Group {
                group_id,
                key_id,
                encrypted,
                key_rotation,
            } => {
                let group_id_typed = ContextGroupId::from(*group_id);

                if let Some(group_key) = load_group_key_by_id(self.store, &group_id_typed, key_id)?
                {
                    self.decrypt_and_apply_group_op(op, &group_id_typed, &group_key, encrypted)?;
                }

                if let Some(rotation) = key_rotation {
                    let ns_id = ContextGroupId::from(op.namespace_id);
                    if let Some(identity) = get_namespace_identity_record(self.store, &ns_id)? {
                        let recipient_sk = PrivateKey::from(identity.private_key);
                        for envelope in &rotation.envelopes {
                            if envelope.recipient == recipient_sk.public_key() {
                                match unwrap_group_key(&recipient_sk, envelope) {
                                    Ok(new_key) => {
                                        let _ =
                                            store_group_key(self.store, &group_id_typed, &new_key)?;
                                        tracing::info!(
                                            group_id = %hex::encode(group_id),
                                            "stored rotated group key"
                                        );
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            ?e,
                                            "failed to unwrap key rotation envelope"
                                        );
                                        result.key_unwrap_failures.push(KeyUnwrapFailure {
                                            group_id: *group_id,
                                            reason: format!("key rotation unwrap failed: {e}"),
                                        });
                                    }
                                }
                                break;
                            }
                        }
                    }
                }
            }
        }

        let delta_id = op
            .content_hash()
            .map_err(|e| eyre::eyre!("content_hash: {e}"))?;
        let head = self.read_head_record()?;
        self.advance_dag_head(delta_id, &op.parent_op_hashes, head.next_nonce)?;
        self.store_operation(op)?;

        Ok(result)
    }

    pub async fn sign_apply_and_publish(
        &self,
        node_client: &calimero_node_primitives::client::NodeClient,
        signer_sk: &PrivateKey,
        op: NamespaceOp,
    ) -> EyreResult<()> {
        let head = self.read_head_record()?;
        let signed = SignedNamespaceOp::sign(
            signer_sk,
            self.namespace_id,
            head.parent_hashes,
            [0u8; 32],
            head.next_nonce,
            op,
        )?;
        let delta_id = signed
            .content_hash()
            .map_err(|e| eyre::eyre!("content_hash: {e}"))?;
        let parent_ids = signed.parent_op_hashes.clone();

        self.apply_signed_op(&signed)?;

        let bytes = borsh::to_vec(&signed).map_err(|e| eyre::eyre!("borsh: {e}"))?;
        node_client
            .publish_signed_namespace_op(self.namespace_id, delta_id, parent_ids, bytes)
            .await
    }

    pub async fn sign_and_publish_without_apply(
        &self,
        node_client: &calimero_node_primitives::client::NodeClient,
        signer_sk: &PrivateKey,
        op: NamespaceOp,
    ) -> EyreResult<()> {
        let head = self.read_head_record()?;
        let signed = SignedNamespaceOp::sign(
            signer_sk,
            self.namespace_id,
            head.parent_hashes,
            [0u8; 32],
            head.next_nonce,
            op,
        )?;
        let delta_id = signed
            .content_hash()
            .map_err(|e| eyre::eyre!("content_hash: {e}"))?;
        let parent_ids = signed.parent_op_hashes.clone();

        self.store_operation(&signed)?;
        self.advance_dag_head(delta_id, &parent_ids, head.next_nonce)?;

        let bytes = borsh::to_vec(&signed).map_err(|e| eyre::eyre!("borsh: {e}"))?;
        node_client
            .publish_signed_namespace_op(self.namespace_id, delta_id, parent_ids, bytes)
            .await
    }

    fn retry_encrypted_ops_for_group(&self, group_id: [u8; 32]) -> EyreResult<()> {
        let gid_typed = ContextGroupId::from(group_id);
        let retry_service = NamespaceRetryService::new(self.store, self.namespace_id);
        let retry_candidates = retry_service.collect_retry_candidates_for_group(group_id)?;
        let attempted = retry_candidates.len();
        if attempted > 0 {
            record_namespace_retry_event("collected");
        }

        for candidate in &retry_candidates {
            let NamespaceOp::Group { ref encrypted, .. } = candidate.signed_op.op else {
                continue;
            };
            match self.decrypt_and_apply_group_op(
                &candidate.signed_op,
                &gid_typed,
                &candidate.group_key,
                encrypted,
            ) {
                Ok(()) => {
                    record_namespace_retry_event("applied");
                    tracing::info!(
                        group_id = %hex::encode(group_id),
                        "retried encrypted op after KeyDelivery"
                    );
                }
                Err(e) => {
                    record_namespace_retry_event("failed");
                    tracing::warn!(
                        group_id = %hex::encode(group_id),
                        ?e,
                        "failed to retry encrypted op after KeyDelivery"
                    );
                }
            }
        }

        if attempted == 0 {
            record_namespace_retry_event("none");
        }

        Ok(())
    }

    fn decrypt_and_apply_group_op(
        &self,
        ns_op: &SignedNamespaceOp,
        group_id: &ContextGroupId,
        group_key: &[u8; 32],
        encrypted: &EncryptedGroupOp,
    ) -> EyreResult<()> {
        let inner_op = decrypt_group_op(group_key, encrypted)?;

        let signed_group_op = SignedGroupOp {
            version: calimero_context_client::local_governance::SIGNED_GROUP_OP_SCHEMA_VERSION,
            group_id: group_id.to_bytes(),
            parent_op_hashes: ns_op.parent_op_hashes.clone(),
            state_hash: ns_op.state_hash,
            signer: ns_op.signer,
            nonce: ns_op.nonce,
            op: inner_op,
            signature: ns_op.signature,
        };

        self.apply_group_op_inner(group_id, &ns_op.signer, ns_op.nonce, &signed_group_op.op)
    }

    fn apply_group_op_inner(
        &self,
        group_id: &ContextGroupId,
        signer: &PublicKey,
        nonce: u64,
        op: &GroupOp,
    ) -> EyreResult<()> {
        let last = get_local_gov_nonce(self.store, group_id, signer)?.unwrap_or(0);
        if nonce <= last {
            tracing::debug!(
                nonce,
                last_nonce = last,
                signer = %signer,
                "ignoring namespace group op with already-processed nonce"
            );
            return Ok(());
        }

        if let GroupOp::ContextRegistered {
            context_id,
            application_id,
            blob_id,
            source,
            service_name,
        } = op
        {
            // Store service_name in a dedicated key so join_context can read it
            // later. We intentionally do NOT write ContextMeta here — that would
            // cause has_context() to return true and skip the bootstrap path
            // (ContextConfig creation, app installation, sync trigger).
            if let Some(name) = service_name {
                super::set_context_service_name(self.store, context_id, name)?;
            }
            if *application_id != ZERO_APPLICATION_ID {
                let app_key = calimero_store::key::ApplicationMeta::new(*application_id);
                let handle = self.store.handle();
                if !handle.has(&app_key)? {
                    drop(handle);
                    let blob_meta = calimero_store::key::BlobMeta::new(*blob_id);
                    let effective_source = if source.starts_with("file://") || source.is_empty() {
                        "calimero://pending-blob-share".to_owned()
                    } else {
                        source.clone()
                    };
                    let stub = calimero_store::types::ApplicationMeta::new(
                        blob_meta,
                        0,
                        effective_source.into_boxed_str(),
                        Vec::new().into_boxed_slice(),
                        blob_meta,
                        String::new().into_boxed_str(),
                        String::new().into_boxed_str(),
                        String::new().into_boxed_str(),
                    );
                    let mut wh = self.store.handle();
                    wh.put(&app_key, &stub)?;
                    tracing::info!(
                        %application_id,
                        blob_id = %blob_id,
                        "created stub application entry from ContextRegistered"
                    );
                }
            }
        }

        let handled = apply_group_op_mutations(self.store, group_id, signer, op)?;
        if !handled {
            tracing::debug!(
                ?op,
                "namespace group op variant not handled by inner apply, stored as skeleton"
            );
        }

        set_local_gov_nonce(self.store, group_id, signer, nonce)?;
        Ok(())
    }

    fn require_namespace_admin(&self, signer: &PublicKey) -> EyreResult<()> {
        let ns_gid = ContextGroupId::from(self.namespace_id);
        if !is_group_admin(self.store, &ns_gid, signer)? {
            bail!(
                "signer {} is not an admin of namespace {}",
                signer,
                hex::encode(self.namespace_id)
            );
        }
        Ok(())
    }

    fn apply_root_op(&self, op: &SignedNamespaceOp, root: &RootOp) -> EyreResult<()> {
        match root {
            RootOp::GroupCreated { group_id } => self.execute_group_created(op, *group_id),
            RootOp::GroupDeleted { group_id } => self.execute_group_deleted(op, *group_id),
            RootOp::AdminChanged { new_admin } => self.execute_admin_changed(op, *new_admin),
            RootOp::PolicyUpdated { .. } => self.execute_policy_updated(op),
            RootOp::GroupNested {
                parent_group_id,
                child_group_id,
            } => self.execute_group_nested(op, *parent_group_id, *child_group_id),
            RootOp::GroupUnnested {
                parent_group_id,
                child_group_id,
            } => self.execute_group_unnested(op, *parent_group_id, *child_group_id),
            RootOp::MemberJoined {
                member,
                signed_invitation,
            } => self.execute_member_joined(op, member, signed_invitation),
            RootOp::KeyDelivery { .. } => Ok(()),
        }
    }

    fn execute_group_created(&self, op: &SignedNamespaceOp, group_id: [u8; 32]) -> EyreResult<()> {
        self.require_namespace_admin(&op.signer)?;
        let gid = ContextGroupId::from(group_id);
        if load_group_meta(self.store, &gid)?.is_some() {
            tracing::debug!(
                group_id = %hex::encode(group_id),
                "group already exists, ignoring GroupCreated"
            );
            return Ok(());
        }

        let meta = calimero_store::key::GroupMetaValue {
            admin_identity: op.signer,
            target_application_id: calimero_primitives::application::ApplicationId::from([0u8; 32]),
            app_key: [0u8; 32],
            upgrade_policy: calimero_primitives::context::UpgradePolicy::default(),
            migration: None,
            created_at: 0,
            auto_join: false,
        };
        save_group_meta(self.store, &gid, &meta)?;
        Ok(())
    }

    fn execute_group_deleted(&self, op: &SignedNamespaceOp, group_id: [u8; 32]) -> EyreResult<()> {
        self.require_namespace_admin(&op.signer)?;
        let gid = ContextGroupId::from(group_id);
        if count_group_contexts(self.store, &gid)? > 0 {
            bail!("cannot delete group: contexts still registered");
        }
        super::delete_group_meta(self.store, &gid)?;
        Ok(())
    }

    fn execute_admin_changed(
        &self,
        op: &SignedNamespaceOp,
        new_admin: PublicKey,
    ) -> EyreResult<()> {
        self.require_namespace_admin(&op.signer)?;
        let ns_gid = ContextGroupId::from(self.namespace_id);
        let mut meta = load_group_meta(self.store, &ns_gid)?
            .ok_or_else(|| eyre::eyre!("namespace root group not found"))?;
        meta.admin_identity = new_admin;
        save_group_meta(self.store, &ns_gid, &meta)?;
        Ok(())
    }

    fn execute_policy_updated(&self, op: &SignedNamespaceOp) -> EyreResult<()> {
        self.require_namespace_admin(&op.signer)?;
        tracing::debug!("PolicyUpdated: stored in DAG log, no additional state mutation");
        Ok(())
    }

    fn execute_group_nested(
        &self,
        op: &SignedNamespaceOp,
        parent_group_id: [u8; 32],
        child_group_id: [u8; 32],
    ) -> EyreResult<()> {
        self.require_namespace_admin(&op.signer)?;
        let parent = ContextGroupId::from(parent_group_id);
        let child = ContextGroupId::from(child_group_id);
        if load_group_meta(self.store, &parent)?.is_none() {
            bail!("parent group not found for nesting");
        }
        if load_group_meta(self.store, &child)?.is_none() {
            bail!("child group not found for nesting");
        }
        nest_group(self.store, &parent, &child)?;
        Ok(())
    }

    fn execute_group_unnested(
        &self,
        op: &SignedNamespaceOp,
        parent_group_id: [u8; 32],
        child_group_id: [u8; 32],
    ) -> EyreResult<()> {
        self.require_namespace_admin(&op.signer)?;
        let parent = ContextGroupId::from(parent_group_id);
        let child = ContextGroupId::from(child_group_id);
        unnest_group(self.store, &parent, &child)?;
        Ok(())
    }

    fn execute_member_joined(
        &self,
        op: &SignedNamespaceOp,
        member: &PublicKey,
        signed_invitation: &calimero_context_config::types::SignedGroupOpenInvitation,
    ) -> EyreResult<()> {
        NamespaceMembershipService::new(self.store, self.namespace_id).apply_member_joined(
            &op.signer,
            member,
            signed_invitation,
        )
    }
}

pub fn apply_signed_namespace_op(
    store: &Store,
    op: &SignedNamespaceOp,
) -> EyreResult<ApplyNamespaceOpResult> {
    NamespaceGovernance::new(store, op.namespace_id).apply_signed_op(op)
}

pub async fn sign_apply_and_publish_namespace_op(
    store: &Store,
    node_client: &calimero_node_primitives::client::NodeClient,
    namespace_id: [u8; 32],
    signer_sk: &PrivateKey,
    op: NamespaceOp,
) -> EyreResult<()> {
    NamespaceGovernance::new(store, namespace_id)
        .sign_apply_and_publish(node_client, signer_sk, op)
        .await
}

pub async fn sign_and_publish_namespace_op(
    store: &Store,
    node_client: &calimero_node_primitives::client::NodeClient,
    namespace_id: [u8; 32],
    signer_sk: &PrivateKey,
    op: NamespaceOp,
) -> EyreResult<()> {
    NamespaceGovernance::new(store, namespace_id)
        .sign_and_publish_without_apply(node_client, signer_sk, op)
        .await
}

pub fn collect_skeleton_delta_ids_for_group(
    store: &Store,
    namespace_id: [u8; 32],
    group_id: [u8; 32],
) -> EyreResult<Vec<[u8; 32]>> {
    NamespaceGovernance::new(store, namespace_id).collect_skeleton_delta_ids_for_group(group_id)
}
