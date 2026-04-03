use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::key::FromKeyParts;
use calimero_store::key::{
    AsKeyParts, GroupMemberValue, GroupMetaValue, GroupOpHeadValue, GroupUpgradeValue,
};
use calimero_store::types::PredefinedEntry;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use calimero_context_client::local_governance::{NamespaceOp, SignedNamespaceOp};

mod aliases;
mod capabilities;
mod context_registration;
mod contexts;
mod group_governance_publisher;
mod group_keys;
mod group_settings;
mod local_state;
mod membership;
mod membership_policy;
mod meta;
mod migrations;
mod namespace;
mod namespace_dag;
mod namespace_governance;
mod namespace_membership;
mod namespace_op_log;
mod namespace_retry;
mod permission_checker;
mod signing_keys;
mod tee;
mod upgrades;
use self::local_state::{append_op_log_entry, set_op_head};

pub use self::aliases::{
    build_namespace_summary, count_group_contexts, delete_all_member_aliases, delete_group_alias,
    enumerate_group_contexts_with_aliases, enumerate_member_aliases, get_context_alias,
    get_group_alias, get_member_alias, set_context_alias, set_group_alias, set_member_alias,
};
pub use self::capabilities::{
    delete_all_member_capabilities, delete_default_capabilities, delete_default_visibility,
    enumerate_member_capabilities, get_context_member_capability, get_default_capabilities,
    get_default_visibility, get_member_capability, set_context_member_capability,
    set_default_capabilities, set_default_visibility, set_member_capability,
};
pub use self::context_registration::ContextRegistrationService;
pub use self::contexts::{
    cascade_remove_member_from_group_tree, enumerate_group_contexts, find_local_signing_identity,
    get_group_for_context, register_context_in_group, unregister_context_from_group,
};
pub use self::group_governance_publisher::GroupGovernancePublisher;
pub use self::group_keys::{
    build_key_rotation, compute_key_id, decrypt_group_op, encrypt_group_op, load_current_group_key,
    load_current_group_key_record, load_group_key_by_id, store_group_key, unwrap_group_key,
    wrap_group_key_for_member, GroupKeyring, StoredGroupKey,
};
pub use self::group_settings::GroupSettingsService;
pub use self::local_state::{
    delete_group_local_rows, get_local_gov_nonce, get_member_context_joins, get_op_head,
    read_op_log_after, remove_all_member_context_joins, set_local_gov_nonce,
    track_member_context_join,
};
pub use self::membership::{
    add_group_member, add_group_member_with_keys, check_group_membership, count_group_admins,
    count_group_members, get_group_member_role, get_group_member_value, is_direct_group_admin,
    is_group_admin, is_group_admin_or_has_capability, list_group_members, remove_group_member,
    require_group_admin, require_group_admin_or_capability,
};
pub use self::membership_policy::MembershipPolicy;
pub use self::meta::{
    compute_group_state_hash, delete_group_meta, enumerate_all_groups, load_group_meta,
    save_group_meta,
};
pub use self::migrations::{
    delete_all_context_last_migrations, get_context_last_migration, set_context_last_migration,
};
pub use self::namespace::{
    collect_descendant_groups, compute_namespace_governance_epoch, create_recursive_invitations,
    get_namespace_identity, get_namespace_identity_record, get_or_create_namespace_identity,
    get_or_create_namespace_identity_bundle, get_parent_group, is_read_only_for_context,
    list_child_groups, nest_group, recursive_remove_member, resolve_namespace,
    resolve_namespace_identity, resolve_namespace_identity_record, store_namespace_identity,
    unnest_group, NamespaceIdentityRecord, ResolvedNamespaceIdentity,
};
pub use self::namespace_dag::{NamespaceDagService, NamespaceHead};
pub use self::namespace_governance::{
    apply_signed_namespace_op, collect_skeleton_delta_ids_for_group, sign_and_publish_namespace_op,
    sign_apply_and_publish_namespace_op, ApplyNamespaceOpResult, KeyUnwrapFailure,
    NamespaceGovernance, PendingKeyDelivery,
};
pub use self::namespace_membership::NamespaceMembershipService;
pub use self::namespace_op_log::NamespaceOpLogService;
pub use self::namespace_retry::NamespaceRetryService;
pub use self::permission_checker::PermissionChecker;
pub use self::signing_keys::{
    delete_all_group_signing_keys, delete_group_signing_key, get_group_signing_key,
    require_group_signing_key, store_group_signing_key,
};
pub use self::tee::{is_quote_hash_used, read_tee_admission_policy, TeeAdmissionPolicy};
pub use self::upgrades::{
    delete_group_upgrade, enumerate_in_progress_upgrades, load_group_upgrade, save_group_upgrade,
};

#[cfg(test)]
use self::upgrades::extract_application_id;

// ---------------------------------------------------------------------------
// Typed errors for group store operations
// ---------------------------------------------------------------------------

/// Structured errors for group store operations.
///
/// Replaces ad-hoc `bail!("string")` errors with matchable variants so callers
/// can programmatically handle specific failure cases (e.g. treating `StaleNonce`
/// as idempotent success rather than an error).
#[derive(Debug, thiserror::Error)]
pub enum GroupStoreError {
    #[error("group {0} not found")]
    GroupNotFound(String),

    #[error("{identity} is not an admin of group {group_id}")]
    NotAdmin { group_id: String, identity: String },

    #[error("cannot remove the last admin of the group")]
    LastAdmin,

    #[error("cannot demote the last admin of the group")]
    LastAdminDemotion,

    #[error("nesting would create a cycle")]
    NestingCycle,

    #[error("group {0} already has a parent; unnest it first")]
    AlreadyHasParent(String),

    #[error("cannot nest a group under itself")]
    SelfNesting,

    #[error("state_hash mismatch: op signed against {expected}, current is {actual}")]
    StateHashMismatch { expected: String, actual: String },

    #[error("nonce {nonce} already processed (last: {last})")]
    StaleNonce { nonce: u64, last: u64 },

    #[error("namespace identity not found for {0}")]
    NoNamespaceIdentity(String),

    #[error("no group key stored for group {0}")]
    NoGroupKey(String),

    #[error("signing key not found for {identity} in group {group_id}")]
    NoSigningKey { group_id: String, identity: String },

    #[error("requester lacks permission to {operation} in group {group_id}")]
    Unauthorized { group_id: String, operation: String },

    #[error("unsupported group op variant")]
    UnsupportedOp,

    #[error("{0}")]
    Other(String),
}

// ---------------------------------------------------------------------------
// Prefix-scan helpers (DRY: replaces 15+ copy-pasted iteration loops)
// ---------------------------------------------------------------------------

/// Collect all keys of type `K` starting at `start`, iterating while the
/// first key byte equals `prefix_byte` and `belongs` returns `true`.
///
/// This helper eliminates the repeated seek-iterate-prefix-check boilerplate
/// used throughout this module.
fn collect_keys_with_prefix<K>(
    store: &Store,
    start: K,
    prefix_byte: u8,
    belongs: impl Fn(&K) -> bool,
) -> EyreResult<Vec<K>>
where
    K: PredefinedEntry + FromKeyParts + AsKeyParts,
    eyre::Report: From<calimero_store::iter::IterError<<K as FromKeyParts>::Error>>,
    for<'a> <K::Codec as calimero_store::entry::Codec<'a, K::DataType<'a>>>::Error:
        std::error::Error + Send + Sync + 'static,
{
    let handle = store.handle();
    let mut iter = handle.iter::<K>()?;
    let first = iter.seek(start).transpose();
    let mut keys = Vec::new();

    for key_result in first.into_iter().chain(iter.keys()) {
        let key = key_result?;
        if key.as_key().as_bytes()[0] != prefix_byte {
            break;
        }
        if !belongs(&key) {
            break;
        }
        keys.push(key);
    }

    Ok(keys)
}

// ---------------------------------------------------------------------------
// GroupHandle — scoped handle for a single group's store operations
// ---------------------------------------------------------------------------

/// A scoped handle binding a `Store` reference and a `ContextGroupId`.
///
/// Provides methods for all single-group operations, eliminating the need
/// to pass `(&Store, &ContextGroupId)` to every free function. Handlers
/// create a `GroupHandle` once and call methods on it.
///
/// The existing free functions remain available for callers that haven't
/// migrated yet.
pub struct GroupHandle<'a> {
    store: &'a Store,
    group_id: ContextGroupId,
}

impl<'a> GroupHandle<'a> {
    pub fn new(store: &'a Store, group_id: ContextGroupId) -> Self {
        Self { store, group_id }
    }

    pub fn store(&self) -> &Store {
        self.store
    }
    pub fn group_id(&self) -> &ContextGroupId {
        &self.group_id
    }

    // --- Meta ---
    pub fn load_meta(&self) -> EyreResult<Option<GroupMetaValue>> {
        load_group_meta(self.store, &self.group_id)
    }
    pub fn save_meta(&self, meta: &GroupMetaValue) -> EyreResult<()> {
        save_group_meta(self.store, &self.group_id, meta)
    }
    pub fn delete_meta(&self) -> EyreResult<()> {
        delete_group_meta(self.store, &self.group_id)
    }
    pub fn compute_state_hash(&self) -> EyreResult<[u8; 32]> {
        compute_group_state_hash(self.store, &self.group_id)
    }

    // --- Members ---
    pub fn add_member(&self, identity: &PublicKey, role: GroupMemberRole) -> EyreResult<()> {
        add_group_member(self.store, &self.group_id, identity, role)
    }
    pub fn add_member_with_keys(
        &self,
        identity: &PublicKey,
        role: GroupMemberRole,
        private_key: Option<[u8; 32]>,
        sender_key: Option<[u8; 32]>,
    ) -> EyreResult<()> {
        add_group_member_with_keys(
            self.store,
            &self.group_id,
            identity,
            role,
            private_key,
            sender_key,
        )
    }
    pub fn remove_member(&self, identity: &PublicKey) -> EyreResult<()> {
        remove_group_member(self.store, &self.group_id, identity)
    }
    pub fn list_members(
        &self,
        offset: usize,
        limit: usize,
    ) -> EyreResult<Vec<(PublicKey, GroupMemberRole)>> {
        list_group_members(self.store, &self.group_id, offset, limit)
    }
    pub fn count_members(&self) -> EyreResult<usize> {
        count_group_members(self.store, &self.group_id)
    }
    pub fn count_admins(&self) -> EyreResult<usize> {
        count_group_admins(self.store, &self.group_id)
    }
    pub fn is_member(&self, identity: &PublicKey) -> EyreResult<bool> {
        check_group_membership(self.store, &self.group_id, identity)
    }
    pub fn get_member_value(&self, identity: &PublicKey) -> EyreResult<Option<GroupMemberValue>> {
        get_group_member_value(self.store, &self.group_id, identity)
    }
    pub fn get_member_role(&self, identity: &PublicKey) -> EyreResult<Option<GroupMemberRole>> {
        get_group_member_role(self.store, &self.group_id, identity)
    }

    // --- Authorization ---
    pub fn is_admin(&self, identity: &PublicKey) -> EyreResult<bool> {
        is_group_admin(self.store, &self.group_id, identity)
    }
    pub fn is_direct_admin(&self, identity: &PublicKey) -> EyreResult<bool> {
        is_direct_group_admin(self.store, &self.group_id, identity)
    }
    pub fn require_admin(&self, identity: &PublicKey) -> EyreResult<()> {
        require_group_admin(self.store, &self.group_id, identity)
    }
    pub fn is_admin_or_has_capability(&self, identity: &PublicKey, cap: u32) -> EyreResult<bool> {
        is_group_admin_or_has_capability(self.store, &self.group_id, identity, cap)
    }
    pub fn require_admin_or_capability(
        &self,
        identity: &PublicKey,
        cap: u32,
        op: &str,
    ) -> EyreResult<()> {
        require_group_admin_or_capability(self.store, &self.group_id, identity, cap, op)
    }

    // --- Nonce ---
    pub fn get_local_gov_nonce(&self, signer: &PublicKey) -> EyreResult<Option<u64>> {
        get_local_gov_nonce(self.store, &self.group_id, signer)
    }
    pub fn set_local_gov_nonce(&self, signer: &PublicKey, nonce: u64) -> EyreResult<()> {
        set_local_gov_nonce(self.store, &self.group_id, signer, nonce)
    }

    // --- Op log ---
    pub fn get_op_head(&self) -> EyreResult<Option<GroupOpHeadValue>> {
        get_op_head(self.store, &self.group_id)
    }
    pub fn read_op_log_after(
        &self,
        after_sequence: u64,
        limit: usize,
    ) -> EyreResult<Vec<(u64, Vec<u8>)>> {
        read_op_log_after(self.store, &self.group_id, after_sequence, limit)
    }

    // --- Governance ops ---
    pub fn apply_signed_op(&self, op: &SignedGroupOp) -> EyreResult<()> {
        apply_local_signed_group_op(self.store, op)
    }
    pub fn sign_apply_op(&self, signer_sk: &PrivateKey, op: GroupOp) -> EyreResult<SignedOpOutput> {
        sign_apply_local_group_op_borsh(self.store, &self.group_id, signer_sk, op)
    }

    // --- Signing keys ---
    pub fn store_signing_key(&self, pk: &PublicKey, sk: &[u8; 32]) -> EyreResult<()> {
        store_group_signing_key(self.store, &self.group_id, pk, sk)
    }
    pub fn get_signing_key(&self, pk: &PublicKey) -> EyreResult<Option<[u8; 32]>> {
        get_group_signing_key(self.store, &self.group_id, pk)
    }
    pub fn delete_signing_key(&self, pk: &PublicKey) -> EyreResult<()> {
        delete_group_signing_key(self.store, &self.group_id, pk)
    }
    pub fn require_signing_key(&self, pk: &PublicKey) -> EyreResult<()> {
        require_group_signing_key(self.store, &self.group_id, pk)
    }

    // --- Group keys ---
    pub fn store_key(&self, group_key: &[u8; 32]) -> EyreResult<[u8; 32]> {
        store_group_key(self.store, &self.group_id, group_key)
    }
    pub fn load_current_key(&self) -> EyreResult<Option<([u8; 32], [u8; 32])>> {
        load_current_group_key(self.store, &self.group_id)
    }
    pub fn load_key_by_id(&self, key_id: &[u8; 32]) -> EyreResult<Option<[u8; 32]>> {
        load_group_key_by_id(self.store, &self.group_id, key_id)
    }

    // --- Contexts ---
    pub fn register_context(&self, context_id: &ContextId) -> EyreResult<()> {
        register_context_in_group(self.store, &self.group_id, context_id)
    }
    pub fn unregister_context(&self, context_id: &ContextId) -> EyreResult<()> {
        unregister_context_from_group(self.store, &self.group_id, context_id)
    }
    pub fn enumerate_contexts(&self, offset: usize, limit: usize) -> EyreResult<Vec<ContextId>> {
        enumerate_group_contexts(self.store, &self.group_id, offset, limit)
    }
    pub fn count_contexts(&self) -> EyreResult<usize> {
        count_group_contexts(self.store, &self.group_id)
    }

    // --- Aliases ---
    pub fn set_alias(&self, alias: &str) -> EyreResult<()> {
        set_group_alias(self.store, &self.group_id, alias)
    }
    pub fn get_alias(&self) -> EyreResult<Option<String>> {
        get_group_alias(self.store, &self.group_id)
    }
    pub fn set_member_alias(&self, member: &PublicKey, alias: &str) -> EyreResult<()> {
        set_member_alias(self.store, &self.group_id, member, alias)
    }
    pub fn get_member_alias(&self, member: &PublicKey) -> EyreResult<Option<String>> {
        get_member_alias(self.store, &self.group_id, member)
    }
    pub fn set_context_alias(&self, ctx_id: &ContextId, alias: &str) -> EyreResult<()> {
        set_context_alias(self.store, &self.group_id, ctx_id, alias)
    }
    pub fn get_context_alias(&self, ctx_id: &ContextId) -> EyreResult<Option<String>> {
        get_context_alias(self.store, &self.group_id, ctx_id)
    }
    pub fn enumerate_contexts_with_aliases(
        &self,
        offset: usize,
        limit: usize,
    ) -> EyreResult<Vec<(ContextId, Option<String>)>> {
        enumerate_group_contexts_with_aliases(self.store, &self.group_id, offset, limit)
    }
    pub fn enumerate_member_aliases(&self) -> EyreResult<Vec<(PublicKey, String)>> {
        enumerate_member_aliases(self.store, &self.group_id)
    }

    // --- Capabilities ---
    pub fn get_member_capability(&self, member: &PublicKey) -> EyreResult<Option<u32>> {
        get_member_capability(self.store, &self.group_id, member)
    }
    pub fn set_member_capability(&self, member: &PublicKey, caps: u32) -> EyreResult<()> {
        set_member_capability(self.store, &self.group_id, member, caps)
    }
    pub fn enumerate_capabilities(&self) -> EyreResult<Vec<(PublicKey, u32)>> {
        enumerate_member_capabilities(self.store, &self.group_id)
    }
    pub fn get_default_capabilities(&self) -> EyreResult<Option<u32>> {
        get_default_capabilities(self.store, &self.group_id)
    }
    pub fn set_default_capabilities(&self, caps: u32) -> EyreResult<()> {
        set_default_capabilities(self.store, &self.group_id, caps)
    }
    pub fn get_default_visibility(&self) -> EyreResult<Option<u8>> {
        get_default_visibility(self.store, &self.group_id)
    }
    pub fn set_default_visibility(&self, mode: u8) -> EyreResult<()> {
        set_default_visibility(self.store, &self.group_id, mode)
    }

    // --- Tree ---
    pub fn parent(&self) -> EyreResult<Option<ContextGroupId>> {
        get_parent_group(self.store, &self.group_id)
    }
    pub fn list_children(&self) -> EyreResult<Vec<ContextGroupId>> {
        list_child_groups(self.store, &self.group_id)
    }
    pub fn collect_descendants(&self) -> EyreResult<Vec<ContextGroupId>> {
        collect_descendant_groups(self.store, &self.group_id)
    }

    // --- Upgrades ---
    pub fn save_upgrade(&self, upgrade: &GroupUpgradeValue) -> EyreResult<()> {
        save_group_upgrade(self.store, &self.group_id, upgrade)
    }
    pub fn load_upgrade(&self) -> EyreResult<Option<GroupUpgradeValue>> {
        load_group_upgrade(self.store, &self.group_id)
    }
    pub fn delete_upgrade(&self) -> EyreResult<()> {
        delete_group_upgrade(self.store, &self.group_id)
    }

    // --- Member-context tracking ---
    pub fn track_member_context_join(
        &self,
        member: &PublicKey,
        context_id: &ContextId,
        context_identity: [u8; 32],
    ) -> EyreResult<()> {
        track_member_context_join(
            self.store,
            &self.group_id,
            member,
            context_id,
            context_identity,
        )
    }
    pub fn get_member_context_joins(
        &self,
        member: &PublicKey,
    ) -> EyreResult<Vec<(ContextId, [u8; 32])>> {
        get_member_context_joins(self.store, &self.group_id, member)
    }
    pub fn remove_all_member_context_joins(
        &self,
        member: &PublicKey,
    ) -> EyreResult<Vec<(ContextId, [u8; 32])>> {
        remove_all_member_context_joins(self.store, &self.group_id, member)
    }

    // --- Cleanup ---
    pub fn delete_all_local_rows(&self) -> EyreResult<()> {
        delete_group_local_rows(self.store, &self.group_id)
    }

    // --- Namespace resolution ---
    pub fn resolve_namespace(&self) -> EyreResult<ContextGroupId> {
        resolve_namespace(self.store, &self.group_id)
    }
    pub fn resolve_namespace_identity(
        &self,
    ) -> EyreResult<Option<(PublicKey, [u8; 32], [u8; 32])>> {
        resolve_namespace_identity(self.store, &self.group_id)
    }
    pub fn get_or_create_namespace_identity(
        &self,
    ) -> EyreResult<(ContextGroupId, PublicKey, [u8; 32], [u8; 32])> {
        get_or_create_namespace_identity(self.store, &self.group_id)
    }

    // --- Migration tracking ---
    pub fn get_context_last_migration(&self, context_id: &ContextId) -> EyreResult<Option<String>> {
        get_context_last_migration(self.store, &self.group_id, context_id)
    }
    pub fn set_context_last_migration(
        &self,
        context_id: &ContextId,
        method: &str,
    ) -> EyreResult<()> {
        set_context_last_migration(self.store, &self.group_id, context_id, method)
    }

    // --- Per-context capabilities ---
    pub fn set_context_member_capability(
        &self,
        ctx_id: &ContextId,
        member: &PublicKey,
        caps: u8,
    ) -> EyreResult<()> {
        set_context_member_capability(self.store, &self.group_id, ctx_id, member, caps)
    }
    pub fn get_context_member_capability(
        &self,
        ctx_id: &ContextId,
        member: &PublicKey,
    ) -> EyreResult<Option<u8>> {
        get_context_member_capability(self.store, &self.group_id, ctx_id, member)
    }
}

// ---------------------------------------------------------------------------
// NamespaceHandle — scoped handle for namespace governance operations
// ---------------------------------------------------------------------------

/// A scoped handle for namespace-level operations (identity, DAG heads, governance ops).
pub struct NamespaceHandle<'a> {
    store: &'a Store,
    namespace_id: [u8; 32],
}

impl<'a> NamespaceHandle<'a> {
    pub fn new(store: &'a Store, namespace_id: [u8; 32]) -> Self {
        Self {
            store,
            namespace_id,
        }
    }

    pub fn namespace_id(&self) -> [u8; 32] {
        self.namespace_id
    }

    pub fn get_identity(&self) -> EyreResult<Option<(PublicKey, [u8; 32], [u8; 32])>> {
        get_namespace_identity(self.store, &ContextGroupId::from(self.namespace_id))
    }

    pub fn store_identity(
        &self,
        pk: &PublicKey,
        sk: &[u8; 32],
        sender: &[u8; 32],
    ) -> EyreResult<()> {
        store_namespace_identity(
            self.store,
            &ContextGroupId::from(self.namespace_id),
            pk,
            sk,
            sender,
        )
    }

    pub fn read_head(&self) -> EyreResult<(Vec<[u8; 32]>, u64)> {
        NamespaceGovernance::new(self.store, self.namespace_id).read_head()
    }

    pub fn advance_dag_head(
        &self,
        delta_id: [u8; 32],
        parent_ids: &[[u8; 32]],
        sequence: u64,
    ) -> EyreResult<()> {
        NamespaceGovernance::new(self.store, self.namespace_id)
            .advance_dag_head(delta_id, parent_ids, sequence)
    }

    pub fn store_gov_op(&self, op: &SignedNamespaceOp) -> EyreResult<()> {
        NamespaceGovernance::new(self.store, self.namespace_id).store_operation(op)
    }

    pub fn apply_signed_op(&self, op: &SignedNamespaceOp) -> EyreResult<ApplyNamespaceOpResult> {
        NamespaceGovernance::new(self.store, self.namespace_id).apply_signed_op(op)
    }
}

// ---------------------------------------------------------------------------
// GovernancePublisher — sign + publish workflow for group/namespace ops
// ---------------------------------------------------------------------------

/// Encapsulates the sign-apply-publish workflow for governance operations.
///
/// Binds a `Store` and `NodeClient` reference, providing methods to publish
/// group ops (encrypted under the namespace topic) and namespace ops.
pub struct GovernancePublisher<'a> {
    store: &'a Store,
    node_client: &'a calimero_node_primitives::client::NodeClient,
}

impl<'a> GovernancePublisher<'a> {
    pub fn new(
        store: &'a Store,
        node_client: &'a calimero_node_primitives::client::NodeClient,
    ) -> Self {
        Self { store, node_client }
    }

    pub async fn publish_group_op(
        &self,
        group_id: &ContextGroupId,
        signer_sk: &PrivateKey,
        op: GroupOp,
    ) -> EyreResult<()> {
        sign_apply_and_publish(self.store, self.node_client, group_id, signer_sk, op).await
    }

    pub async fn publish_group_removal(
        &self,
        group_id: &ContextGroupId,
        signer_sk: &PrivateKey,
        removed_member: &PublicKey,
    ) -> EyreResult<()> {
        sign_apply_and_publish_removal(
            self.store,
            self.node_client,
            group_id,
            signer_sk,
            removed_member,
        )
        .await
    }

    pub async fn publish_namespace_op(
        &self,
        namespace_id: [u8; 32],
        signer_sk: &PrivateKey,
        op: NamespaceOp,
    ) -> EyreResult<()> {
        NamespaceGovernance::new(self.store, namespace_id)
            .sign_apply_and_publish(self.node_client, signer_sk, op)
            .await
    }

    pub async fn publish_namespace_op_without_apply(
        &self,
        namespace_id: [u8; 32],
        signer_sk: &PrivateKey,
        op: NamespaceOp,
    ) -> EyreResult<()> {
        NamespaceGovernance::new(self.store, namespace_id)
            .sign_and_publish_without_apply(self.node_client, signer_sk, op)
            .await
    }
}

// ---------------------------------------------------------------------------
// GroupStoreIndex — cross-group queries and handle factory
// ---------------------------------------------------------------------------

/// Top-level entry point for group store operations.
///
/// Provides cross-group queries (enumerate all, find group for context) and
/// acts as a factory for [`GroupHandle`] and [`NamespaceHandle`].
pub struct GroupStoreIndex<'a> {
    store: &'a Store,
}

impl<'a> GroupStoreIndex<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    pub fn group(&self, group_id: ContextGroupId) -> GroupHandle<'a> {
        GroupHandle::new(self.store, group_id)
    }

    pub fn namespace(&self, namespace_id: [u8; 32]) -> NamespaceHandle<'a> {
        NamespaceHandle::new(self.store, namespace_id)
    }

    pub fn enumerate_all(
        &self,
        offset: usize,
        limit: usize,
    ) -> EyreResult<Vec<([u8; 32], GroupMetaValue)>> {
        enumerate_all_groups(self.store, offset, limit)
    }

    pub fn get_group_for_context(
        &self,
        context_id: &ContextId,
    ) -> EyreResult<Option<ContextGroupId>> {
        get_group_for_context(self.store, context_id)
    }

    pub fn enumerate_in_progress_upgrades(
        &self,
    ) -> EyreResult<Vec<(ContextGroupId, GroupUpgradeValue)>> {
        enumerate_in_progress_upgrades(self.store)
    }

    pub fn resolve_namespace(&self, group_id: &ContextGroupId) -> EyreResult<ContextGroupId> {
        resolve_namespace(self.store, group_id)
    }

    pub fn is_read_only_for_context(
        &self,
        context_id: &ContextId,
        identity: &PublicKey,
    ) -> EyreResult<bool> {
        is_read_only_for_context(self.store, context_id, identity)
    }

    pub fn find_local_signing_identity(
        &self,
        context_id: &ContextId,
    ) -> EyreResult<Option<PublicKey>> {
        find_local_signing_identity(self.store, context_id)
    }
}

/// Maximum number of parent hashes allowed in a single [`SignedGroupOp`].
/// Chosen to allow realistic merge breadth (multi-admin concurrent ops) while
/// bounding memory/CPU cost during signature verification and storage.
const MAX_PARENT_OP_HASHES: usize = 256;

/// Maximum DAG heads before forcing a synthetic merge. Prevents unbounded
/// growth from many concurrent admins operating without merges.
const MAX_DAG_HEADS: usize = 64;

/// Apply the mutation described by a [`GroupOp`] to the local store.
///
/// Handles authorization checks and state mutations for ALL `GroupOp` variants.
/// Returns `Ok(true)` if the op was handled, `Ok(false)` if the variant is not
/// recognized (callers decide whether to error or log).
///
/// This is the single source of truth for group governance mutations -- both
/// the signed-op path and the namespace-op-inner path delegate here.
fn apply_group_op_mutations(
    store: &Store,
    group_id: &ContextGroupId,
    signer: &PublicKey,
    op: &GroupOp,
) -> EyreResult<bool> {
    let permissions = PermissionChecker::new(store, *group_id);
    let membership_policy = MembershipPolicy::new(store, *group_id);
    let settings = GroupSettingsService::new(store, *group_id);
    let context_registration = ContextRegistrationService::new(store, *group_id);

    match op {
        GroupOp::Noop => {}
        GroupOp::MemberAdded { member, role } => {
            permissions.require_manage_members(signer, "add member")?;
            permissions.require_admin_to_add_admin(signer, role)?;
            add_group_member(store, group_id, member, role.clone())?;
        }
        GroupOp::MemberRemoved { member } => {
            permissions.require_manage_members(signer, "remove member")?;
            permissions.require_admin_to_remove_admin(signer, member)?;
            membership_policy.ensure_not_last_admin_removal(member)?;
            cascade_remove_member_from_group_tree(store, group_id, member)?;
            remove_group_member(store, group_id, member)?;
        }
        GroupOp::MemberRoleSet { member, role } => {
            permissions.require_admin(signer)?;
            membership_policy.ensure_not_last_admin_demotion(member, role)?;
            add_group_member(store, group_id, member, role.clone())?;
        }
        GroupOp::MemberCapabilitySet {
            member,
            capabilities,
        } => {
            permissions.require_admin(signer)?;
            set_member_capability(store, group_id, member, *capabilities)?;
        }
        GroupOp::DefaultCapabilitiesSet { capabilities } => {
            settings.set_default_capabilities(signer, *capabilities)?;
        }
        GroupOp::UpgradePolicySet { policy } => {
            settings.set_upgrade_policy(signer, policy)?;
        }
        GroupOp::TargetApplicationSet {
            app_key,
            target_application_id,
        } => {
            settings.set_target_application(signer, app_key, target_application_id)?;
        }
        GroupOp::ContextRegistered {
            context_id,
            application_id,
            ..
        } => {
            context_registration.register(&permissions, signer, context_id, application_id)?;
        }
        GroupOp::ContextDetached { context_id } => {
            context_registration.detach(&permissions, signer, context_id)?;
        }
        GroupOp::DefaultVisibilitySet { mode } => {
            settings.set_default_visibility(signer, *mode)?;
        }
        GroupOp::ContextAliasSet { context_id, alias } => {
            permissions.require_admin(signer)?;
            set_context_alias(store, group_id, context_id, alias)?;
        }
        GroupOp::MemberAliasSet { member, alias } => {
            permissions.require_admin_or_self(signer, member)?;
            set_member_alias(store, group_id, member, alias)?;
        }
        GroupOp::GroupAliasSet { alias } => {
            settings.set_group_alias(signer, alias)?;
        }
        GroupOp::GroupDelete => {
            permissions.require_admin(signer)?;
            if count_group_contexts(store, group_id)? > 0 {
                bail!("cannot delete group: one or more contexts are still registered");
            }
            delete_group_local_rows(store, group_id)?;
        }
        GroupOp::GroupMigrationSet { migration } => {
            settings.set_group_migration(signer, migration)?;
        }
        GroupOp::ContextCapabilityGranted {
            context_id,
            member,
            capability,
        } => {
            permissions.require_manage_members(signer, "grant context capability")?;
            let current =
                get_context_member_capability(store, group_id, context_id, member)?.unwrap_or(0);
            set_context_member_capability(
                store,
                group_id,
                context_id,
                member,
                current | capability,
            )?;
        }
        GroupOp::ContextCapabilityRevoked {
            context_id,
            member,
            capability,
        } => {
            permissions.require_manage_members(signer, "revoke context capability")?;
            let current =
                get_context_member_capability(store, group_id, context_id, member)?.unwrap_or(0);
            set_context_member_capability(
                store,
                group_id,
                context_id,
                member,
                current & !capability,
            )?;
        }
        GroupOp::TeeAdmissionPolicySet { .. } => {
            permissions.require_admin(signer)?;
        }
        GroupOp::MemberJoinedViaTeeAttestation {
            member,
            quote_hash: _,
            mrtd,
            rtmr0,
            rtmr1,
            rtmr2,
            rtmr3,
            tcb_status,
            role,
        } => {
            membership_policy.require_tee_attestation_verifier_membership(signer)?;
            let policy = membership_policy.read_required_tee_admission_policy()?;
            membership_policy.validate_tee_attestation_allowlists(
                &policy, mrtd, rtmr0, rtmr1, rtmr2, rtmr3, tcb_status,
            )?;
            membership_policy.admit_member_if_absent(member, role)?;
        }
        #[allow(unreachable_patterns)]
        _ => return Ok(false),
    }

    Ok(true)
}

/// Apply a [`SignedGroupOp`] to the local group store (signature, monotonic nonce, admin rules).
///
/// # Concurrency
///
/// Callers must serialize access per `group_id`. In the node this is guaranteed
/// by the single-threaded actix `ContextManager` actor which processes messages
/// sequentially. Direct concurrent calls from multiple threads for the same
/// group are **not** safe and could produce duplicate sequence numbers.
///
/// # Parent validation
///
/// `parent_op_hashes` are not validated against the current `dag_heads` — an op
/// may reference ancestors further back in the DAG. This is acceptable because
/// authorization is checked against the *current* group state (not the parent
/// state), and the `DagStore` performs topological ordering independently.
pub fn apply_local_signed_group_op(store: &Store, op: &SignedGroupOp) -> EyreResult<()> {
    if op.parent_op_hashes.len() > MAX_PARENT_OP_HASHES {
        bail!(
            "too many parent_op_hashes ({}, max {})",
            op.parent_op_hashes.len(),
            MAX_PARENT_OP_HASHES
        );
    }
    op.verify_signature()
        .map_err(|e| eyre::eyre!("signed group op: {e}"))?;
    let group_id = ContextGroupId::from(op.group_id);

    let zero_hash = [0u8; 32];
    if op.state_hash != zero_hash {
        let current_state_hash = compute_group_state_hash(store, &group_id)?;
        if op.state_hash != current_state_hash {
            tracing::debug!(
                group_id = %hex::encode(group_id.to_bytes()),
                expected = %hex::encode(op.state_hash),
                actual = %hex::encode(current_state_hash),
                nonce = op.nonce,
                signer = %op.signer,
                "rejecting op: state_hash mismatch (signed against stale state)"
            );
            bail!(
                "state_hash mismatch: op was signed against {}, current state is {}",
                hex::encode(op.state_hash),
                hex::encode(current_state_hash)
            );
        }
    }

    let last = get_local_gov_nonce(store, &group_id, &op.signer)?.unwrap_or(0);
    if op.nonce <= last {
        tracing::debug!(
            nonce = op.nonce,
            last_nonce = last,
            signer = %op.signer,
            "ignoring op with already-processed nonce"
        );
        return Ok(());
    }

    if !apply_group_op_mutations(store, &group_id, &op.signer, &op.op)? {
        bail!("unsupported group op variant for local apply");
    }

    let content_hash = op
        .content_hash()
        .map_err(|e| eyre::eyre!("content_hash: {e}"))?;
    let head = get_op_head(store, &group_id)?;
    let next_seq = head.as_ref().map_or(1, |h| h.sequence.saturating_add(1));
    let op_bytes = borsh::to_vec(op).map_err(|e| eyre::eyre!("borsh: {e}"))?;
    append_op_log_entry(store, &group_id, next_seq, &op_bytes)?;

    let parent_set: std::collections::HashSet<[u8; 32]> =
        op.parent_op_hashes.iter().copied().collect();
    let mut new_heads: Vec<[u8; 32]> = head
        .map(|h| h.dag_heads)
        .unwrap_or_default()
        .into_iter()
        .filter(|h| !parent_set.contains(h))
        .collect();
    new_heads.push(content_hash);
    if new_heads.len() > MAX_DAG_HEADS {
        let excess = new_heads.len() - MAX_DAG_HEADS;
        tracing::warn!(
            group_id = %hex::encode(group_id.to_bytes()),
            dropped = excess,
            remaining = MAX_DAG_HEADS,
            "DAG heads exceeded cap, dropping oldest heads"
        );
        new_heads.drain(..excess);
    }
    set_op_head(store, &group_id, next_seq, new_heads)?;

    set_local_gov_nonce(store, &group_id, &op.signer, op.nonce)?;

    Ok(())
}

/// Output of [`sign_apply_local_group_op_borsh`] for publishing via gossip.
pub struct SignedOpOutput {
    pub bytes: Vec<u8>,
    pub delta_id: [u8; 32],
    pub parent_ids: Vec<[u8; 32]>,
}

/// Sign the next monotonic [`SignedGroupOp`] for `signer_sk`, apply via [`apply_local_signed_group_op`],
/// and return a [`SignedOpOutput`] with serialized bytes and DAG metadata for callers.
pub fn sign_apply_local_group_op_borsh(
    store: &Store,
    group_id: &ContextGroupId,
    signer_sk: &PrivateKey,
    op: GroupOp,
) -> EyreResult<SignedOpOutput> {
    let last = get_local_gov_nonce(store, group_id, &signer_sk.public_key())?.unwrap_or(0);
    let nonce = last
        .checked_add(1)
        .ok_or_else(|| eyre::eyre!("group governance nonce overflow"))?;
    let parent_hashes = get_op_head(store, group_id)?
        .map(|h| h.dag_heads.clone())
        .unwrap_or_default();
    let state_hash = compute_group_state_hash(store, group_id)?;
    let signed = SignedGroupOp::sign(
        signer_sk,
        group_id.to_bytes(),
        parent_hashes,
        state_hash,
        nonce,
        op,
    )?;
    let delta_id = signed
        .content_hash()
        .map_err(|e| eyre::eyre!("content_hash: {e}"))?;
    let parent_ids = signed.parent_op_hashes.clone();
    apply_local_signed_group_op(store, &signed)?;
    let bytes = borsh::to_vec(&signed).map_err(|e| eyre::eyre!("borsh: {e}"))?;
    Ok(SignedOpOutput {
        bytes,
        delta_id,
        parent_ids,
    })
}

/// Sign a group governance op, apply it locally, and publish it on the
/// namespace topic as an encrypted `NamespaceOp::Group`.
///
/// This is the main entry point for all group mutations (add member,
/// set capabilities, etc.) that need to reach other namespace members.
///
/// When `removed_member` is `Some`, a key rotation is generated and attached
/// to the namespace op so the removed member loses access to future ops.
pub async fn sign_apply_and_publish(
    store: &Store,
    node_client: &calimero_node_primitives::client::NodeClient,
    group_id: &ContextGroupId,
    signer_sk: &PrivateKey,
    op: GroupOp,
) -> EyreResult<()> {
    GroupGovernancePublisher::new(store, node_client, *group_id)
        .sign_apply_and_publish(signer_sk, op)
        .await
}

/// Like [`sign_apply_and_publish`] but attaches a [`KeyRotation`] bundle to
/// the encrypted `MemberRemoved` op. Generates a new group key, wraps it for
/// all remaining members, and stores the new key locally.
pub async fn sign_apply_and_publish_removal(
    store: &Store,
    node_client: &calimero_node_primitives::client::NodeClient,
    group_id: &ContextGroupId,
    signer_sk: &PrivateKey,
    removed_member: &PublicKey,
) -> EyreResult<()> {
    GroupGovernancePublisher::new(store, node_client, *group_id)
        .sign_apply_and_publish_removal(signer_sk, removed_member)
        .await
}

/// Cascade `target_application_id` and `app_key` to all descendant subgroups
/// starting from `group_id` downward (breadth-first, bounded by
/// `MAX_NAMESPACE_DEPTH`).
///
/// Called when a group's application is upgraded via governance. Each
/// descendant's metadata is updated in a separate write. If the process is
/// interrupted mid-cascade, some descendants may still reference the old
/// application -- the next upgrade attempt will bring them up to date.
// ---------------------------------------------------------------------------
// Invitation commitment helpers
// ---------------------------------------------------------------------------

/// In the namespace model, application cascading to child groups is handled
/// at the namespace governance level rather than via tree traversal.
fn cascade_target_application(
    _store: &Store,
    _group_id: &ContextGroupId,
    _target_application_id: &calimero_primitives::application::ApplicationId,
    _app_key: &[u8; 32],
) -> EyreResult<()> {
    Ok(())
}

#[cfg(test)]
mod tests;
