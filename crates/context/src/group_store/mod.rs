use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_primitives::metadata::{validate_metadata_payload, MetadataRecord};
use calimero_store::key::FromKeyParts;
use calimero_store::key::{
    AsKeyParts, GroupMemberValue, GroupMetaValue, GroupOpHeadValue, GroupUpgradeValue,
};
use calimero_store::types::PredefinedEntry;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use calimero_context_client::local_governance::SignedNamespaceOp;

mod capabilities;
mod context_registration;
mod context_tree;
mod contexts;
mod deny_list;
mod governance_signer;
mod group_governance_publisher;
mod group_keys;
mod group_settings;
mod local_state;
mod membership;
mod meta;
mod metadata;
mod migrations;
mod namespace;
mod permission_checker;
mod signing_keys;
mod tee;
mod upgrades;
use self::local_state::persist_group_governance_progress;

pub use self::capabilities::{
    delete_all_member_capabilities, delete_default_capabilities, delete_subgroup_visibility,
    enumerate_member_capabilities, get_context_member_capability, get_default_capabilities,
    get_member_capability, get_subgroup_visibility, is_open_chain_to_namespace,
    set_context_member_capability, set_default_capabilities, set_member_capability,
    set_subgroup_visibility,
};
pub use self::context_registration::ContextRegistrationService;
pub use self::context_tree::ContextTreeService;
pub use self::contexts::{
    cascade_remove_member_from_group_tree, enumerate_group_contexts, find_local_signing_identity,
    get_group_for_context, is_currently_authorized_for_context, register_context_in_group,
    restore_member_context_identities, unregister_context_from_group,
};
pub use self::deny_list::{
    clear_all_denied, clear_denied, is_author_denied_for_context, is_denied, mark_denied,
};
pub use self::governance_signer::GovernanceSigner;
pub use self::group_governance_publisher::GroupGovernancePublisher;
pub use self::group_keys::{
    build_key_rotation, compute_key_id, decrypt_group_op, encrypt_group_op, load_current_group_key,
    load_current_group_key_record, load_group_key_by_id, store_group_key, unwrap_group_key,
    wrap_group_key_for_member, GroupKeyring, StoredGroupKey,
};
pub use self::group_settings::GroupSettingsService;
pub use self::local_state::{
    delete_group_local_rows, delete_namespace_local_state, get_local_gov_nonce,
    get_member_context_joins, get_op_head, read_op_log_after, remove_all_member_context_joins,
    set_local_gov_nonce, track_member_context_join,
};
pub use self::membership::{
    add_group_member, add_group_member_with_keys, check_group_membership,
    check_group_membership_path, count_group_admins, count_group_members,
    enumerate_inherited_members, get_effective_member_capabilities, get_group_member_role,
    get_group_member_value, has_direct_group_member, is_authoritative_namespace_identity,
    is_direct_group_admin, is_group_admin, is_group_admin_or_has_capability, is_inherited_admin,
    list_group_members, membership_status_at, namespace_member_pubkeys, remove_group_member,
    require_group_admin, require_group_admin_or_capability, set_member_auto_follow,
    subgroup_visible_to, trusted_anchors_for_group, GroupMembershipView, MembershipPath,
    MembershipPolicy, MembershipStatus,
};
pub use self::meta::{
    compute_group_state_hash, compute_group_state_hash_after_remove, delete_group_meta,
    enumerate_all_groups, load_group_meta, save_group_meta, snapshot_context_state_hashes,
};
pub use self::metadata::{
    build_namespace_summary, count_group_contexts, delete_all_member_metadata,
    delete_context_metadata, delete_group_metadata, delete_member_metadata,
    enumerate_group_contexts_with_names, enumerate_member_metadata, get_context_metadata,
    get_group_metadata, get_member_metadata, set_context_metadata, set_group_metadata,
    set_member_metadata,
};
pub use self::migrations::{
    delete_all_context_last_migrations, get_context_last_migration, set_context_last_migration,
};
pub(crate) use self::namespace::MAX_NAMESPACE_DEPTH;
pub use self::namespace::{
    apply_signed_namespace_op, collect_descendant_groups, collect_skeleton_delta_ids_for_group,
    collect_subtree_for_cascade, collect_visible_descendant_groups, create_recursive_invitations,
    get_namespace_identity, get_namespace_identity_record, get_or_create_namespace_identity,
    get_or_create_namespace_identity_bundle, get_parent_group, is_authorized_for_context_state_op,
    is_descendant_of, is_read_only_for_context, list_child_groups, nest_group,
    recursive_remove_member, reparent_group, resolve_namespace, resolve_namespace_identity,
    resolve_namespace_identity_record, sign_and_publish_namespace_op,
    sign_apply_and_publish_namespace_op, store_namespace_identity, unnest_group,
    ApplyNamespaceOpResult, CascadePayload, KeyUnwrapFailure, NamespaceDagService,
    NamespaceGovernance, NamespaceHead, NamespaceIdentityRecord, NamespaceMembershipService,
    NamespaceOpLogService, NamespaceRetryService, PendingKeyDelivery, ReparentOutcome,
    ResolvedNamespaceIdentity,
};
pub use self::permission_checker::PermissionChecker;
pub use self::signing_keys::{
    delete_all_group_signing_keys, delete_group_signing_key, get_group_signing_key,
    require_group_signing_key, resolve_group_signing_key, store_group_signing_key,
};
pub use self::tee::{
    is_quote_hash_used, is_tee_admitted_identity, read_tee_admission_policy, TeeAdmissionPolicy,
};
pub use self::upgrades::{
    delete_group_upgrade, enumerate_in_progress_upgrades, load_group_upgrade, save_group_upgrade,
};

#[cfg(test)]
use self::local_state::{append_op_log_entry, set_op_head};
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

/// Collect a page of keys matching a prefix without materializing the full
/// key-space first.
fn collect_keys_with_prefix_paginated<K>(
    store: &Store,
    start: K,
    prefix_byte: u8,
    belongs: impl Fn(&K) -> bool,
    offset: usize,
    limit: usize,
) -> EyreResult<Vec<K>>
where
    K: PredefinedEntry + FromKeyParts + AsKeyParts,
    eyre::Report: From<calimero_store::iter::IterError<<K as FromKeyParts>::Error>>,
    for<'a> <K::Codec as calimero_store::entry::Codec<'a, K::DataType<'a>>>::Error:
        std::error::Error + Send + Sync + 'static,
{
    if limit == 0 {
        return Ok(Vec::new());
    }

    let handle = store.handle();
    let mut iter = handle.iter::<K>()?;
    let first = iter.seek(start).transpose();
    let mut seen = 0usize;
    let mut keys = Vec::new();

    for key_result in first.into_iter().chain(iter.keys()) {
        let key = key_result?;
        if key.as_key().as_bytes()[0] != prefix_byte {
            break;
        }
        if !belongs(&key) {
            break;
        }
        if seen < offset {
            seen += 1;
            continue;
        }
        keys.push(key);
        if keys.len() >= limit {
            break;
        }
    }

    Ok(keys)
}

/// Count keys matching a prefix without storing all matching keys in memory.
fn count_keys_with_prefix<K>(
    store: &Store,
    start: K,
    prefix_byte: u8,
    belongs: impl Fn(&K) -> bool,
) -> EyreResult<usize>
where
    K: PredefinedEntry + FromKeyParts + AsKeyParts,
    eyre::Report: From<calimero_store::iter::IterError<<K as FromKeyParts>::Error>>,
    for<'a> <K::Codec as calimero_store::entry::Codec<'a, K::DataType<'a>>>::Error:
        std::error::Error + Send + Sync + 'static,
{
    let handle = store.handle();
    let mut iter = handle.iter::<K>()?;
    let first = iter.seek(start).transpose();
    let mut count = 0usize;

    for key_result in first.into_iter().chain(iter.keys()) {
        let key = key_result?;
        if key.as_key().as_bytes()[0] != prefix_byte {
            break;
        }
        if !belongs(&key) {
            break;
        }
        count += 1;
    }

    Ok(count)
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

    // --- Metadata records ---
    pub fn set_metadata(&self, record: &MetadataRecord) -> EyreResult<()> {
        set_group_metadata(self.store, &self.group_id, record)
    }
    pub fn get_metadata(&self) -> EyreResult<Option<MetadataRecord>> {
        get_group_metadata(self.store, &self.group_id)
    }
    pub fn set_member_metadata(
        &self,
        member: &PublicKey,
        record: &MetadataRecord,
    ) -> EyreResult<()> {
        set_member_metadata(self.store, &self.group_id, member, record)
    }
    pub fn get_member_metadata(&self, member: &PublicKey) -> EyreResult<Option<MetadataRecord>> {
        get_member_metadata(self.store, &self.group_id, member)
    }
    pub fn set_context_metadata(
        &self,
        ctx_id: &ContextId,
        record: &MetadataRecord,
    ) -> EyreResult<()> {
        set_context_metadata(self.store, &self.group_id, ctx_id, record)
    }
    pub fn get_context_metadata(&self, ctx_id: &ContextId) -> EyreResult<Option<MetadataRecord>> {
        get_context_metadata(self.store, &self.group_id, ctx_id)
    }
    pub fn enumerate_contexts_with_names(
        &self,
        offset: usize,
        limit: usize,
    ) -> EyreResult<Vec<(ContextId, Option<String>)>> {
        enumerate_group_contexts_with_names(self.store, &self.group_id, offset, limit)
    }
    pub fn enumerate_member_metadata(&self) -> EyreResult<Vec<(PublicKey, MetadataRecord)>> {
        enumerate_member_metadata(self.store, &self.group_id)
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
    pub fn get_subgroup_visibility(&self) -> EyreResult<calimero_context_config::VisibilityMode> {
        get_subgroup_visibility(self.store, &self.group_id)
    }
    pub fn set_subgroup_visibility(
        &self,
        mode: calimero_context_config::VisibilityMode,
    ) -> EyreResult<()> {
        set_subgroup_visibility(self.store, &self.group_id, mode)
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

/// Wall-clock milliseconds since the Unix epoch. Used to stamp
/// [`MetadataRecord::updated_at`] at apply time (and when a handler seeds an
/// initial metadata record on group create/join). This is informational only
/// (it is deliberately excluded from `compute_group_state_hash`), so a small
/// per-peer skew is acceptable.
pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Compare local post-apply state hashes against the values signed
/// into `MemberRemoved` / `MemberLeft` and emit a structured warn log
/// on mismatch. Does NOT roll back the apply or return an error — the
/// signed op is already valid (signature verified earlier), and the
/// admin's view is canonical by definition. The mismatch is a signal
/// that this receiver's state has diverged from the canonical view,
/// to be resolved by reconcile-via-anchor sync (a future PR); until
/// then the warn line is what surfaces the divergence to operators.
///
/// Two failure modes deliberately accept-and-log rather than error:
///
/// 1. **Hash computation fails.** Reading the post-apply state to
///    recompute the hash should not fail under normal conditions; if
///    it does we log the failure and move on — the apply already
///    happened, no rollback is possible at this layer.
/// 2. **Empty signed values per field.** A sender that didn't
///    precompute the signed claims (older op shapes from before the
///    signed-claim wire change, or test helpers using placeholder
///    values) signs zeros; comparing against a real post-apply hash
///    would spuriously trigger a mismatch every time. The two fields
///    are checked independently: an all-zero `expected_group_state_hash`
///    skips only the group-state comparison, an empty
///    `expected_context_state_hashes` skips only the per-context
///    comparison.
///
///    **Ambiguity acknowledged:** an empty list is "no claim" from a
///    legacy sender AND "no contexts to claim" from a current
///    sender on a context-less group. The two cases are
///    indistinguishable on the wire, so the per-context check is
///    skipped in both. This is acceptable because a context-less
///    group has nothing per-context to compare anyway — the empty
///    actual would match the empty expected. The group-state half
///    still runs and catches any membership-row drift, which is the
///    interesting failure mode for context-less groups. Wrapping the
///    field in `Option<Vec<...>>` to disambiguate would be a
///    wire-format churn for no behavioral gain.
///
///    **Bounded residual**: if a group transitions from "has
///    contexts" to "has no contexts" (every registered context was
///    detached), a removal signed in that state would carry an
///    empty list. A receiver that hasn't yet applied the matching
///    `ContextDetached` ops still has its registrations and the
///    per-context check would be skipped on it. This is not a
///    silent gap — the receiver's namespace DAG heads disagree with
///    the signer's `SignedNamespaceOp.parent_op_hashes` in this
///    scenario, so the cross-DAG membership check on subsequent
///    state deltas returns `Unknown { needed }` and buffers them
///    until the detach ops arrive (or anchor-sync reconciles). The
///    hash check skip is therefore additive to an existing detection
///    path, not the sole defense.
///
///    Once the network has fully rolled forward to signed-claim ops
///    the zero sentinel becomes dead and can be removed; the
///    empty-list "no contexts" case stays valid forever.
/// Re-export of the cross-crate `DivergenceReport` carried inside
/// `NamespaceApplyOutcome::Applied`. Defined in `calimero-context-client`
/// because the message type that carries it lives there, and
/// primitives can't depend on this crate. Internal apply-path users
/// in `group_store` import it via this re-export.
///
/// `None` means no divergence detected (or no claim made by the
/// signed op). `Some` means at least one of {group state hash,
/// per-context state hash, set-membership of registered contexts}
/// diverged from what the signer signed against. The reconcile-
/// via-anchor path consumes the `hash_differs` list (carrying the
/// **signed expected** hash) to verify received state before adopt.
pub use calimero_context_client::messages::DivergenceReport;

fn verify_post_apply_state_hashes(
    store: &Store,
    group_id: &ContextGroupId,
    op_kind: &'static str,
    expected_group_state_hash: &[u8; 32],
    expected_context_state_hashes: &[(ContextId, [u8; 32])],
) -> Option<DivergenceReport> {
    let check_group_hash = *expected_group_state_hash != [0u8; 32];
    let check_context_hashes = !expected_context_state_hashes.is_empty();
    if !check_group_hash && !check_context_hashes {
        return None;
    }

    // Group-state half. `None` here means the check was either
    // skipped (no claim) or the recompute itself errored — in both
    // cases we suppress the hash fields from the divergence warn so
    // an operator doesn't see misleading `0000…` values.
    let group_outcome: Option<(bool, [u8; 32])> = if check_group_hash {
        match compute_group_state_hash(store, group_id) {
            Ok(actual) => Some((actual != *expected_group_state_hash, actual)),
            Err(err) => {
                tracing::warn!(
                    group_id = %hex::encode(group_id.to_bytes()),
                    op_kind,
                    %err,
                    "post-apply group-state hash recompute failed; skipping group-state check"
                );
                // `None` here means this half couldn't run — must NOT
                // `return None` here, because the per-context half may
                // still confirm divergence below and surface a report
                // the reconcile path needs. Bailing the whole function
                // on a half-error silently drops that signal.
                None
            }
        }
    } else {
        None
    };

    // Per-context half. A snapshot error must NOT fall through to
    // the diff with an empty `actual` — that would report every
    // signed expected context as divergent (false-positive storm).
    // `None` here means the check was skipped or the snapshot
    // errored; the warn omits the per-context fields in that case.
    let context_diff: Option<ContextHashDiff> = if check_context_hashes {
        match snapshot_context_state_hashes(store, group_id) {
            Ok(actual_context_state_hashes) => Some(diff_sorted_context_hashes(
                group_id,
                op_kind,
                expected_context_state_hashes,
                &actual_context_state_hashes,
            )),
            Err(err) => {
                tracing::warn!(
                    group_id = %hex::encode(group_id.to_bytes()),
                    op_kind,
                    %err,
                    "post-apply per-context state-hash snapshot failed; skipping per-context \
                     convergence check"
                );
                // Same reason as the group-state error path: do NOT
                // bail the whole function. If the group-state half
                // already confirmed divergence above we still need
                // to surface a report so reconcile can fire.
                None
            }
        }
    } else {
        None
    };

    let group_diverges = matches!(group_outcome, Some((true, _)));
    let context_diverges = context_diff.as_ref().is_some_and(|d| !d.is_empty());
    if !group_diverges && !context_diverges {
        return None;
    }

    // Branch the warn so the operator can tell which half ran AND
    // which kind of context divergence fired. The three context
    // buckets — hash_differs, only_in_expected, only_in_actual —
    // distinguish the partition-window CRDT case (`hash_differs`),
    // the fresh-node-catchup case (`only_in_expected`), and the
    // receiver-ahead case (`only_in_actual`). Operators that see
    // only `only_in_expected` populated on a freshly-joined node
    // can recognise normal bootstrap noise; a populated
    // `hash_differs` is the real signal anchor-sync reconcile
    // consumes.
    let format_ids = |ids: &[ContextId]| -> Vec<String> {
        ids.iter()
            .map(|c| hex::encode(AsRef::<[u8; 32]>::as_ref(c)))
            .collect()
    };
    let format_hash_differs = |entries: &[(ContextId, [u8; 32])]| -> Vec<String> {
        entries
            .iter()
            .map(|(c, _)| hex::encode(AsRef::<[u8; 32]>::as_ref(c)))
            .collect()
    };
    let diff_ref = context_diff.as_ref();
    let group_actual_for_log = group_outcome.map(|(_, h)| h);
    match (group_outcome.is_some(), diff_ref) {
        (true, Some(diff)) => {
            tracing::warn!(
                group_id = %hex::encode(group_id.to_bytes()),
                op_kind,
                group_hash_diverges = group_diverges,
                expected_group_state_hash = %hex::encode(expected_group_state_hash),
                actual_group_state_hash = %hex::encode(group_actual_for_log.unwrap_or([0u8; 32])),
                hash_differs_count = diff.hash_differs.len(),
                only_in_expected_count = diff.only_in_expected.len(),
                only_in_actual_count = diff.only_in_actual.len(),
                hash_differs = ?format_hash_differs(&diff.hash_differs),
                only_in_expected = ?format_ids(&diff.only_in_expected),
                only_in_actual = ?format_ids(&diff.only_in_actual),
                "cross-DAG state-hash divergence detected on apply — reconcile-via-anchor will heal"
            );
        }
        (true, None) => {
            tracing::warn!(
                group_id = %hex::encode(group_id.to_bytes()),
                op_kind,
                group_hash_diverges = group_diverges,
                expected_group_state_hash = %hex::encode(expected_group_state_hash),
                actual_group_state_hash = %hex::encode(group_actual_for_log.unwrap_or([0u8; 32])),
                per_context_check = "skipped",
                "cross-DAG group-state hash divergence detected on apply — reconcile-via-anchor will heal"
            );
        }
        (false, Some(diff)) => {
            tracing::warn!(
                group_id = %hex::encode(group_id.to_bytes()),
                op_kind,
                group_hash_check = "skipped",
                hash_differs_count = diff.hash_differs.len(),
                only_in_expected_count = diff.only_in_expected.len(),
                only_in_actual_count = diff.only_in_actual.len(),
                hash_differs = ?format_hash_differs(&diff.hash_differs),
                only_in_expected = ?format_ids(&diff.only_in_expected),
                only_in_actual = ?format_ids(&diff.only_in_actual),
                "cross-DAG per-context state-hash divergence detected on apply — reconcile-via-anchor will heal"
            );
        }
        (false, None) => {
            // Both halves errored — each error path already emitted
            // its own warn upstream, so nothing to add here. We also
            // return `None` for divergence: an errored recompute is
            // not the same as a confirmed divergence, and triggering
            // a reconcile here would burn anchor bandwidth on every
            // transient store hiccup.
            return None;
        }
    }

    // Return the structured report so the apply path can route it
    // to the reconcile-via-anchor handler. The report carries the
    // signed expected per-context hashes so the reconcile path
    // verifies adopted state against them — not against the
    // anchor's claim, which could be lying.
    Some(DivergenceReport {
        group_id: *group_id,
        op_kind,
        group_hash_diverges: group_diverges,
        hash_differs: context_diff
            .as_ref()
            .map(|d| d.hash_differs.clone())
            .unwrap_or_default(),
        only_in_expected: context_diff
            .as_ref()
            .map(|d| d.only_in_expected.clone())
            .unwrap_or_default(),
        only_in_actual: context_diff.map(|d| d.only_in_actual).unwrap_or_default(),
    })
}

/// Linear merge-scan of two `(ContextId, [u8; 32])` slices both
/// pre-sorted by `ContextId`. Returns a [`ContextHashDiff`] grouping
/// divergent ids by category — hash differs, only in expected,
/// only in actual. O(n) time and O(divergent.len()) space; replaces
/// an earlier two-`BTreeMap` approach that was O(n log n) on an
/// apply-time hot path.
///
/// Emits a debug log per id for the only-in-actual and only-in-expected
/// paths. The "only in expected" case is the dominant noise source on
/// freshly-joined nodes whose `ContextMeta` rows haven't been written
/// yet; the parent warn log distinguishes the three buckets so
/// operators can recognise bootstrap noise (only-in-expected populated,
/// hash-differs empty) from real partition-window divergence
/// (hash-differs populated).
/// Categorized divergence report from `diff_sorted_context_hashes`.
/// The three buckets distinguish the three cases an operator cares
/// about in the warn log: a real hash mismatch on a shared context
/// (the partition-window state-DAG case), a context the signer
/// snapshotted that the receiver hasn't materialized (fresh-node
/// catchup, expected noise), and a context the receiver materialized
/// after the signer signed (receiver-ahead, also expected noise).
///
/// `hash_differs` carries the **expected** hash alongside each
/// divergent `ContextId`. The reconcile-via-anchor path consumes
/// this pair: target a sync at `context_id`, then verify the
/// received root hash against `expected_hash` before adopting.
pub struct ContextHashDiff {
    pub hash_differs: Vec<(ContextId, [u8; 32])>,
    pub only_in_expected: Vec<ContextId>,
    pub only_in_actual: Vec<ContextId>,
}

impl ContextHashDiff {
    pub fn is_empty(&self) -> bool {
        self.hash_differs.is_empty()
            && self.only_in_expected.is_empty()
            && self.only_in_actual.is_empty()
    }
}

fn diff_sorted_context_hashes(
    group_id: &ContextGroupId,
    op_kind: &'static str,
    expected: &[(ContextId, [u8; 32])],
    actual: &[(ContextId, [u8; 32])],
) -> ContextHashDiff {
    // The merge-scan is only correct when both inputs are sorted by
    // `ContextId`. `actual` comes from `snapshot_context_state_hashes`
    // which sorts before returning. `expected` rides on a signed op
    // whose deterministic content hash requires the sender's
    // `snapshot_context_state_hashes` to have sorted before signing,
    // so a peer that didn't sort would have produced a different
    // op content hash and been dedup'd / rejected at the wire layer.
    // The assertion catches dev / test misuse where the contract is
    // violated before it becomes a quiet divergence-report bug.
    debug_assert!(
        expected.windows(2).all(|w| w[0].0 < w[1].0),
        "expected context-hash snapshot must be strictly sorted by ContextId"
    );
    debug_assert!(
        actual.windows(2).all(|w| w[0].0 < w[1].0),
        "actual context-hash snapshot must be strictly sorted by ContextId"
    );
    let mut diff = ContextHashDiff {
        hash_differs: Vec::new(),
        only_in_expected: Vec::new(),
        only_in_actual: Vec::new(),
    };
    let mut i = 0;
    let mut j = 0;
    while i < expected.len() && j < actual.len() {
        let (e_cid, e_hash) = &expected[i];
        let (a_cid, a_hash) = &actual[j];
        match e_cid.cmp(a_cid) {
            std::cmp::Ordering::Equal => {
                if e_hash != a_hash {
                    diff.hash_differs.push((*e_cid, *e_hash));
                }
                i += 1;
                j += 1;
            }
            std::cmp::Ordering::Less => {
                tracing::debug!(
                    group_id = %hex::encode(group_id.to_bytes()),
                    op_kind,
                    context_id = %hex::encode(AsRef::<[u8; 32]>::as_ref(e_cid)),
                    "context in signed snapshot but not materialized locally — \
                     fresh node catchup or partition-window divergence"
                );
                diff.only_in_expected.push(*e_cid);
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                tracing::debug!(
                    group_id = %hex::encode(group_id.to_bytes()),
                    op_kind,
                    context_id = %hex::encode(AsRef::<[u8; 32]>::as_ref(a_cid)),
                    "context materialized locally but not in signed snapshot — \
                     receiver applied a registration the signer's view missed"
                );
                diff.only_in_actual.push(*a_cid);
                j += 1;
            }
        }
    }
    // Tail handling: anything left on either side is one-sided.
    while i < expected.len() {
        let (cid, _) = &expected[i];
        tracing::debug!(
            group_id = %hex::encode(group_id.to_bytes()),
            op_kind,
            context_id = %hex::encode(AsRef::<[u8; 32]>::as_ref(cid)),
            "context in signed snapshot but not materialized locally — \
             fresh node catchup or partition-window divergence"
        );
        diff.only_in_expected.push(*cid);
        i += 1;
    }
    while j < actual.len() {
        let (cid, _) = &actual[j];
        tracing::debug!(
            group_id = %hex::encode(group_id.to_bytes()),
            op_kind,
            context_id = %hex::encode(AsRef::<[u8; 32]>::as_ref(cid)),
            "context materialized locally but not in signed snapshot — \
             receiver applied a registration the signer's view missed"
        );
        diff.only_in_actual.push(*cid);
        j += 1;
    }
    diff
}

/// Apply the mutation described by a [`GroupOp`] to the local store.
/// Synthesize an `OpEvent::AutoFollowSet` for a freshly-written member
/// row when its `auto_follow.contexts` flag is `true`. Called from every
/// apply-site that creates a new `GroupMember` row (admin-add, TEE
/// attestation, Open-subgroup self-join) so the auto-follow handler — which
/// only listens to `AutoFollowSet`, not `MemberAdded` / `MemberJoined` /
/// `TeeMemberAdmitted` — gets a single, uniform trigger for the
/// "joiner has the flag on, backfill the group's contexts" cascade.
///
/// Idempotent: if the member's row was already in the store (e.g. a
/// `MemberAdded` op for someone re-added after a remove), the value
/// reflects whatever was preserved by `add_group_member` and we emit
/// based on that. The handler's downstream `handle_auto_follow_enabled`
/// uses `join_context`, which is itself idempotent.
///
/// Read failures are downgraded to a warn log and `Ok(())` — the
/// apply-site has already written the row and committed by the time
/// this is called, so a transient read failure here should not roll
/// back the op via the caller's `?`. The synthesized event is a
/// best-effort optimisation; missing it means the joiner won't
/// backfill pre-existing contexts in this group, but they'll still
/// auto-follow future ones via the `ContextRegistered` event handler.
pub(super) fn emit_auto_follow_set_if_enabled(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
) -> EyreResult<()> {
    let value = match get_group_member_value(store, group_id, member) {
        Ok(Some(v)) => v,
        Ok(None) => {
            tracing::warn!(
                group_id = %hex::encode(group_id.to_bytes()),
                %member,
                "post-apply read found no member row — skipping auto-follow emission"
            );
            return Ok(());
        }
        Err(err) => {
            // Best-effort: log and continue. See the function-level
            // doc comment for the rationale.
            tracing::warn!(
                group_id = %hex::encode(group_id.to_bytes()),
                %member,
                ?err,
                "post-apply read failed — skipping auto-follow emission"
            );
            return Ok(());
        }
    };
    if value.auto_follow.contexts {
        crate::op_events::notify(crate::op_events::OpEvent::AutoFollowSet {
            group_id: group_id.to_bytes(),
            member: *member,
            contexts: true,
            subgroups: value.auto_follow.subgroups,
        });
    }
    Ok(())
}

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
) -> EyreResult<(bool, Option<DivergenceReport>)> {
    let permissions = PermissionChecker::new(store, *group_id);
    let membership_policy = MembershipPolicy::new(store, *group_id);
    let settings = GroupSettingsService::new(store, *group_id);
    let context_registration = ContextRegistrationService::new(store, *group_id);
    // Filled by the `MemberRemoved` / `MemberLeft` arms when their
    // post-apply hash check reports divergence from the signed
    // claims. The caller forwards this up the apply pipeline; the
    // node-side handler routes it to the reconcile-via-anchor path.
    let mut divergence: Option<DivergenceReport> = None;

    match op {
        GroupOp::Noop => {}
        GroupOp::MemberAdded { member, role } => {
            if *role == GroupMemberRole::ReadOnlyTee {
                bail!("ReadOnlyTee can only be assigned via MemberJoinedViaTeeAttestation");
            }
            permissions.require_manage_members(signer, "add member")?;
            permissions.require_admin_to_add_admin(signer, role)?;
            add_group_member(store, group_id, member, role.clone())?;
            // Clear any stale deny-list entry — re-adding a previously
            // removed member transparently restores their network-level
            // access. Idempotent on a member who was never denied.
            clear_denied(store, group_id, member)?;
            // Restore per-context `ContextIdentity` rows that
            // `cascade_remove_member_from_group_tree` deleted on a prior
            // `MemberRemoved`. The local-rejoiner anti-spoof gate is
            // enforced inside `restore_member_context_identities` — on
            // peers (admin or other members applying this op) it is a
            // no-op. Idempotent on first-time adds: the joiner's later
            // `join_context` sees an existing row and skips.
            restore_member_context_identities(store, group_id, member)?;
            crate::op_events::notify(crate::op_events::OpEvent::MemberAdded {
                group_id: group_id.to_bytes(),
                member: *member,
                role: role.clone(),
            });
            // #2422 Option 2: synthesize an `AutoFollowSet` event whenever
            // a freshly-written member row has `auto_follow.contexts` set
            // (the post-#2422 default). The auto-follow handler subscribes
            // to `AutoFollowSet` (not `MemberAdded`), so without this the
            // joiner would correctly auto-follow FUTURE
            // `OpEvent::ContextRegistered` events but never backfill
            // contexts that already existed in the group at join time —
            // which is the user-reported regression (Ronit/Fran 2026-05-20).
            // The handler short-circuits via `NotForSelf` on every node
            // except the joiner, so the cascade only fires once per
            // membership change.
            emit_auto_follow_set_if_enabled(store, group_id, member)?;
        }
        GroupOp::MemberRemoved {
            member,
            expected_group_state_hash,
            expected_context_state_hashes,
            ..
        } => {
            permissions.require_manage_members(signer, "remove member")?;
            permissions.require_admin_to_remove_admin(signer, member)?;
            // Owner is immune to involuntary removal. Owner must
            // `TransferOwnership` first to step down, then they can be
            // removed (or self-leave once that op exists).
            if let Some(meta) = load_group_meta(store, group_id)? {
                if meta.owner_identity == *member {
                    bail!(
                        "cannot remove owner of group {}; owner must \
                         TransferOwnership to a successor before removal",
                        hex::encode(group_id.to_bytes())
                    );
                }
            }
            membership_policy.ensure_not_last_admin_removal(member)?;
            cascade_remove_member_from_group_tree(store, group_id, member)?;
            remove_group_member(store, group_id, member)?;
            // Add to deny-list: state deltas from this member will be
            // dropped at the receive entry point before the cross-DAG
            // check runs. Cleared if/when the member is re-added.
            mark_denied(store, group_id, member)?;
            // Ordering invariant: `verify_post_apply_state_hashes`
            // must run AFTER the last mutation that touches inputs
            // to `compute_group_state_hash` (i.e. `GroupMeta` rows
            // and `GroupMember` rows for this `group_id`). Of the
            // three preceding steps here only `remove_group_member`
            // touches those inputs:
            //
            // * `cascade_remove_member_from_group_tree` deletes
            //   `ContextIdentity` rows in the state-DAG-layer
            //   column — disjoint from `GroupMember`. Does not
            //   affect the hash.
            // * `mark_denied` writes a `GroupDeniedMember` row — a
            //   separate column. Does not affect the hash.
            // * `remove_group_member` deletes the `GroupMember`
            //   row — this is the step the pre-apply simulation
            //   in `compute_group_state_hash_after_remove` mirrors.
            //
            // Adding any future mutation between
            // `remove_group_member` and this call that DOES touch
            // `GroupMeta` or `GroupMember` rows for `group_id` will
            // make the recomputed hash diverge from the signed
            // claim on every honest receiver. The pre-apply
            // simulation only models the single removal; any extra
            // mutation here is invisible to it.
            divergence = verify_post_apply_state_hashes(
                store,
                group_id,
                "MemberRemoved",
                expected_group_state_hash,
                expected_context_state_hashes,
            );
            crate::op_events::notify(crate::op_events::OpEvent::MemberRemoved {
                group_id: group_id.to_bytes(),
                member: *member,
            });
        }
        GroupOp::MemberRoleSet { member, role } => {
            if *role == GroupMemberRole::ReadOnlyTee {
                bail!("ReadOnlyTee can only be assigned via MemberJoinedViaTeeAttestation");
            }
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
            service_name,
            ..
        } => {
            context_registration.register(&permissions, signer, context_id, application_id)?;
            if let Some(name) = service_name {
                set_context_service_name(store, context_id, name)?;
            }
            // Signal any waiters (e.g. `join_context` racing against gossipsub
            // propagation) that the context→group mapping has just been
            // persisted. See `crate::registration_notify` for rationale.
            crate::registration_notify::notify(*context_id);
            crate::op_events::notify(crate::op_events::OpEvent::ContextRegistered {
                group_id: group_id.to_bytes(),
                context_id: *context_id,
            });
        }
        GroupOp::ContextDetached { context_id } => {
            context_registration.detach(&permissions, signer, context_id)?;
        }
        GroupOp::SubgroupVisibilitySet { mode } => {
            let visibility = match *mode {
                0 => calimero_context_config::VisibilityMode::Open,
                _ => calimero_context_config::VisibilityMode::Restricted,
            };
            settings.set_subgroup_visibility(signer, visibility)?;
        }
        GroupOp::GroupMetadataSet { name, data } => {
            permissions.require_can_manage_metadata(signer)?;
            validate_metadata_payload(name.as_deref(), data).map_err(|e| eyre::eyre!(e))?;
            set_group_metadata(
                store,
                group_id,
                &MetadataRecord {
                    name: name.clone(),
                    data: data.clone(),
                    updated_at: now_millis(),
                    updated_by: *signer,
                },
            )?;
        }
        GroupOp::MemberMetadataSet { member, name, data } => {
            // A member may always set *their own* metadata — but only if they
            // actually are a member of this group; otherwise this is gated like
            // the other metadata ops (admin or CAN_MANAGE_METADATA).
            if signer == member {
                if !check_group_membership(store, group_id, signer)? {
                    bail!(
                        "signer {signer} is not a member of group {}",
                        hex::encode(group_id.to_bytes())
                    );
                }
            } else {
                permissions.require_can_manage_metadata(signer)?;
            }
            validate_metadata_payload(name.as_deref(), data).map_err(|e| eyre::eyre!(e))?;
            set_member_metadata(
                store,
                group_id,
                member,
                &MetadataRecord {
                    name: name.clone(),
                    data: data.clone(),
                    updated_at: now_millis(),
                    updated_by: *signer,
                },
            )?;
        }
        GroupOp::ContextMetadataSet {
            context_id,
            name,
            data,
        } => {
            permissions.require_can_manage_metadata(signer)?;
            // Reject metadata for a context that isn't registered in this
            // group — otherwise we'd create orphaned `GroupContextMetadata`
            // rows for contexts in a different group (or no group at all).
            if get_group_for_context(store, context_id)? != Some(*group_id) {
                bail!(
                    "context {context_id} is not registered in group {}",
                    hex::encode(group_id.to_bytes())
                );
            }
            validate_metadata_payload(name.as_deref(), data).map_err(|e| eyre::eyre!(e))?;
            set_context_metadata(
                store,
                group_id,
                context_id,
                &MetadataRecord {
                    name: name.clone(),
                    data: data.clone(),
                    updated_at: now_millis(),
                    updated_by: *signer,
                },
            )?;
        }
        GroupOp::GroupDelete => {
            // Owner-only. Admins can no longer delete the group on their
            // own — only the owner can. Tightens the previous policy
            // (`require_admin`) which let any admin destroy the group.
            let meta = load_group_meta(store, group_id)?.ok_or_else(|| {
                eyre::eyre!(
                    "cannot delete unknown group {}",
                    hex::encode(group_id.to_bytes())
                )
            })?;
            if meta.owner_identity != *signer {
                bail!(
                    "only the owner of group {} can delete it; \
                     transfer ownership first if a non-owner needs to remove it",
                    hex::encode(group_id.to_bytes())
                );
            }
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
            // TEE policies are namespace-scoped — refuse to apply an op
            // targeting a subgroup even if it arrives via replication.
            // Reader resolves to root anyway, so a stored subgroup op would
            // be dead data; rejecting at apply time keeps state clean.
            if self::namespace::get_parent_group(store, group_id)?.is_some() {
                bail!(
                    "TeeAdmissionPolicySet rejected on subgroup {group_id:?}: policy is \
                     namespace-scoped, set it on the namespace root"
                );
            }
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
            if *role != GroupMemberRole::ReadOnlyTee {
                bail!("MemberJoinedViaTeeAttestation must use ReadOnlyTee role");
            }
            membership_policy.require_tee_attestation_verifier_membership(signer)?;
            let policy = membership_policy.read_required_tee_admission_policy()?;
            membership_policy.validate_tee_attestation_allowlists(
                &policy, mrtd, rtmr0, rtmr1, rtmr2, rtmr3, tcb_status,
            )?;
            membership_policy.admit_member_if_absent(member, role)?;
            // Same rationale as `MemberAdded`: a TEE rejoining after a
            // prior removal should have their deny-list entry cleared.
            clear_denied(store, group_id, member)?;
            crate::op_events::notify(crate::op_events::OpEvent::TeeMemberAdmitted {
                group_id: group_id.to_bytes(),
                member: *member,
            });
            // #2422 Option 2: TEE attestation goes through
            // `admit_member_if_absent` → `add_group_member`, which writes
            // the new default `{contexts: true, subgroups: false}`. The
            // fleet-join sidecar (`crates/server/src/admin/handlers/tee/
            // fleet_join.rs`) then issues an explicit `SetMemberAutoFollow
            // {true, true}` op, which fires its own `AutoFollowSet`. That
            // creates a second cascade — both join_context attempts are
            // idempotent (see auto_follow.rs:101-107), so the only cost
            // is two rate-limiter tokens. Documented and accepted.
            emit_auto_follow_set_if_enabled(store, group_id, member)?;
        }
        GroupOp::MemberSetAutoFollow {
            target,
            auto_follow_contexts,
            auto_follow_subgroups,
        } => {
            // Admin-or-self: admin can toggle flags for any member, a
            // member can toggle their own. Non-admin, non-self attempts
            // are rejected.
            if !permissions.is_admin(signer)? && signer != target {
                bail!("only group admin or the target member can set auto-follow");
            }
            // Target must already be a group member.
            if get_group_member_role(store, group_id, target)?.is_none() {
                bail!("target is not a member of this group");
            }
            let flags = calimero_store::key::AutoFollowFlags {
                contexts: *auto_follow_contexts,
                subgroups: *auto_follow_subgroups,
            };
            set_member_auto_follow(store, group_id, target, flags)?;
            crate::op_events::notify(crate::op_events::OpEvent::AutoFollowSet {
                group_id: group_id.to_bytes(),
                member: *target,
                contexts: *auto_follow_contexts,
                subgroups: *auto_follow_subgroups,
            });
        }
        GroupOp::MemberLeft {
            member,
            expected_group_state_hash,
            expected_context_state_hashes,
            ..
        } => {
            // Self-leave: signer must equal the member being removed.
            // No capability check beyond self-equality — any member can
            // leave themselves without admin involvement.
            if signer != member {
                bail!("MemberLeft is self-leave only: signer must equal the leaving member");
            }

            // Direct-row check. If `signer` is only an inherited member
            // (Open subgroup with no stored row), there's nothing to delete
            // here — they have to leave whichever ancestor anchors their
            // membership instead.
            if get_group_member_role(store, group_id, member)?.is_none() {
                bail!(
                    "member is not a direct member of group {}; \
                     leave the parent group where the membership anchor lives",
                    hex::encode(group_id.to_bytes())
                );
            }

            // Owner cannot self-leave. Must TransferOwnership first.
            if let Some(meta) = load_group_meta(store, group_id)? {
                if meta.owner_identity == *member {
                    bail!(
                        "owner of group {} cannot self-leave; \
                         transfer ownership to a successor first",
                        hex::encode(group_id.to_bytes())
                    );
                }
            }

            // Last-admin protection — same helper used by MemberRemoved.
            membership_policy.ensure_not_last_admin_removal(member)?;

            // Detect namespace-leave: if this group has no parent it IS the
            // namespace, and leaving must cascade through every descendant
            // group where the leaver has a direct row. Per the design's
            // "no cascade for leave_group" rule, non-namespace groups don't
            // touch siblings or descendants. See § 6 for cascade semantics.
            let is_namespace_leave =
                crate::group_store::namespace::resolve_namespace(store, group_id)? == *group_id;

            if is_namespace_leave {
                // Walk subtree, gather descendants where leaver has a direct
                // row. Run owner + last-admin checks across all of them
                // BEFORE mutating anything, so a failure surfaces the
                // offending scope to the user with no half-applied cleanup.
                let descendants =
                    crate::group_store::namespace::collect_descendant_groups(store, group_id)?;

                let mut direct_descendants: Vec<ContextGroupId> = Vec::new();
                for sub in &descendants {
                    if get_group_member_role(store, sub, member)?.is_some() {
                        if let Some(sub_meta) = load_group_meta(store, sub)? {
                            if sub_meta.owner_identity == *member {
                                bail!(
                                    "cannot leave namespace: leaver owns subgroup {}; \
                                     transfer ownership of every owned scope first",
                                    hex::encode(sub.to_bytes())
                                );
                            }
                        }
                        let sub_policy = MembershipPolicy::new(store, *sub);
                        sub_policy.ensure_not_last_admin_removal(member)?;
                        direct_descendants.push(*sub);
                    }
                }

                for sub in &direct_descendants {
                    cascade_remove_member_from_group_tree(store, sub, member)?;
                    remove_group_member(store, sub, member)?;
                    // Self-leave cascade: deny-list every descendant
                    // group where the leaver had a row, so their
                    // state-delta traffic on those topics is dropped
                    // until they re-join.
                    mark_denied(store, sub, member)?;
                    crate::op_events::notify(crate::op_events::OpEvent::MemberRemoved {
                        group_id: sub.to_bytes(),
                        member: *member,
                    });
                }
            }

            cascade_remove_member_from_group_tree(store, group_id, member)?;
            remove_group_member(store, group_id, member)?;
            // Deny-list the leaver on this group too. See
            // `MemberRemoved` for the same rationale.
            mark_denied(store, group_id, member)?;

            // NOTE on forward secrecy: this op deliberately does NOT trigger
            // the key-rotation pipeline that `MemberRemoved` does, because
            // the publisher (the leaver) cannot generate the new key without
            // also retaining it — which would defeat forward secrecy.
            // Proper forward secrecy on self-leave requires a follow-up
            // two-phase rotation (a remaining admin's apply hook publishes
            // KeyDelivery), which is tracked as a follow-up to this PR. For
            // now, an admin-initiated `MemberRemoved` is the path to a
            // cryptographically-complete leave; `MemberLeft` is the
            // governance-level departure (membership row removed, peers
            // observe the leave) without the rotation. Same caveat applies
            // to the namespace cascade above — row-removal only.
            //
            // Ordering invariant (mirrors `MemberRemoved`'s call site):
            // `verify_post_apply_state_hashes` must run after the last
            // mutation that touches `GroupMeta` or `GroupMember` rows
            // for `group_id`. The namespace-leave cascade above operates
            // on DESCENDANT group ids — those mutations don't change
            // `compute_group_state_hash(group_id)`'s inputs (the hash
            // only reads members of THIS group, not descendants). The
            // `remove_group_member(store, group_id, member)` call just
            // above is the only mutation here that affects the hash;
            // `cascade_remove_member_from_group_tree` touches
            // `ContextIdentity` rows and `mark_denied` touches
            // `GroupDeniedMember` rows, both in separate columns. If a
            // future mutation between `remove_group_member` and this
            // call DOES touch `GroupMeta` or `GroupMember` rows for
            // `group_id`, the recomputed hash will diverge from the
            // signer's pre-apply simulation on every honest receiver.
            divergence = verify_post_apply_state_hashes(
                store,
                group_id,
                "MemberLeft",
                expected_group_state_hash,
                expected_context_state_hashes,
            );
            crate::op_events::notify(crate::op_events::OpEvent::MemberRemoved {
                group_id: group_id.to_bytes(),
                member: *member,
            });
        }
        GroupOp::TransferOwnership { new_owner } => {
            // Owner-only — current owner is the only signer who can transfer.
            let mut meta = load_group_meta(store, group_id)?.ok_or_else(|| {
                eyre::eyre!(
                    "cannot transfer ownership of unknown group {}",
                    hex::encode(group_id.to_bytes())
                )
            })?;

            if meta.owner_identity != *signer {
                bail!(
                    "only the current owner of group {} can transfer ownership; \
                     signer is not the owner",
                    hex::encode(group_id.to_bytes())
                );
            }

            // The new owner must already be an Admin of the group. Transfer
            // does not implicitly invite or promote — the successor must
            // already be in place at admin tier. This prevents two awkward
            // states:
            //   * Transferring to a non-member: would create an absentee
            //     owner.
            //   * Transferring to a plain Member: Owner has all Admin
            //     privileges by design (see doc § 7 privilege matrix), so
            //     a plain-Member owner would have a confusing "owner with
            //     reduced capabilities" status. Require Admin first;
            //     promote then transfer if needed.
            match get_group_member_role(store, group_id, new_owner)? {
                Some(GroupMemberRole::Admin) => {}
                Some(other) => bail!(
                    "new owner of group {} must be an Admin, but is currently {:?}; \
                     promote them to Admin before transferring ownership",
                    hex::encode(group_id.to_bytes()),
                    other
                ),
                None => bail!(
                    "new owner is not a member of group {}; invite and promote them first",
                    hex::encode(group_id.to_bytes())
                ),
            }

            meta.owner_identity = *new_owner;
            save_group_meta(store, group_id, &meta)?;
        }
        GroupOp::CascadeTargetApplicationSet {
            from_app_key,
            app_key,
            target_application_id,
        } => {
            // Walk the descendant tree (incl. signed group) and apply the
            // settings mutation to every descendant whose current `app_key`
            // matches `from_app_key`. Heterogeneous descendants (`app_key !=
            // from_app_key`) are silently skipped per spec § 3.2 — that
            // skip is also the optimistic-concurrency guard for two cascade
            // ops racing against the same subtree (spec § 5).
            //
            // The walk is read-only and cycle/depth-bounded; see the
            // `crate::cascade::walk_for_predicate` doc-comment.
            let entries = crate::cascade::walk_for_predicate(store, *group_id, *from_app_key)?;

            // Pre-scan: verify the signer would pass the per-descendant
            // `require_manage_application` check on EVERY matched
            // descendant before issuing any writes. Without this, a
            // descendant deep in the cascade with a stricter capability
            // configuration (e.g. Restricted subgroup where the
            // namespace-level admin signer is not a direct admin) would
            // cause the `set_target_application` `?` mid-loop to abort
            // the whole apply AFTER prior descendants have already been
            // mutated, leaving the store in a partial-cascade state on
            // both emitter and receiver paths.
            for entry in &entries {
                if !entry.matched {
                    continue;
                }
                let entry_permissions = PermissionChecker::new(store, entry.group_id);
                if !entry_permissions.can_manage_application(signer)? {
                    bail!(
                        "cascade target-application set: signer {} lacks MANAGE_APPLICATION on \
                         descendant {}; aborting before any writes to keep cascade atomic",
                        signer,
                        hex::encode(entry.group_id.to_bytes())
                    );
                }
            }

            let mut any_applied = false;
            for entry in entries {
                if !entry.matched {
                    tracing::debug!(
                        target: "calimero::cascade",
                        group_id = %hex::encode(entry.group_id.to_bytes()),
                        from_app_key = %hex::encode(from_app_key),
                        "CascadeTargetApplicationSet: skip (app_key mismatch)"
                    );
                    continue;
                }

                // Reuse the existing single-group settings mutation, scoped
                // to each matched descendant. The pre-scan above already
                // verified `signer` holds `MANAGE_APPLICATION` on every
                // matched descendant, so the `?` here is unreachable in
                // production — kept as a defensive backstop in case the
                // store mutates between the scan and the apply (which
                // can't happen on the single-threaded namespace actor
                // path, but is cheap to leave in place).
                let entry_settings = GroupSettingsService::new(store, entry.group_id);
                entry_settings.set_target_application(signer, app_key, target_application_id)?;

                // Per-context InProgress status + per-context migration
                // propagator dispatch are intentionally NOT performed here:
                // `apply_group_op_mutations` is a sync store-only function
                // (no `ContextClient`, no `NodeClient`, no actor `Context`),
                // and `propagate_upgrade` is an async actor-spawned
                // routine. The cascade-emitting RPC handler
                // (`handlers/upgrade_group.rs`, PR-2 Task 6) is responsible
                // for spawning a `propagate_upgrade` per matched descendant
                // group it cascaded over, mirroring how it already spawns
                // one for the signed root on the single-group path.
                //
                // Peers receiving this op via gossip apply the settings
                // mutation here and then rely on the local write-gate
                // (PR-2 Task 7) to refuse user-initiated writes against
                // contexts whose group's `target_application_id` has been
                // cascaded ahead of their local execution state.

                any_applied = true;
            }
            if !any_applied {
                tracing::debug!(
                    target: "calimero::cascade",
                    signed_group = %hex::encode(group_id.to_bytes()),
                    from_app_key = %hex::encode(from_app_key),
                    "CascadeTargetApplicationSet: no descendants matched"
                );
            }
            // Cascade variants don't produce per-op divergence reports —
            // the only producers today are MemberRemoved/MemberLeft.
            // Fall through to the function-tail `Ok((true, divergence))`
            // exit (rather than an early `return`) so the cascade arms
            // share the same handled-flag convention as every other
            // arm: the variant WAS recognised; a no-match outcome is a
            // successful no-op, not unknown-variant. Returning
            // `handled = false` here would make the caller
            // `apply_local_signed_group_op` bail with
            // "unsupported group op variant for local apply", which
            // also breaks the concurrent-cascade safety case (loser
            // cascade arrives with `from_app_key` no longer matching
            // anything and is intended to be silently swallowed) AND
            // the audit-log persistence path in
            // `namespace/governance.rs` (only persists when
            // `handled == true`).
            divergence = None;
        }
        GroupOp::CascadeGroupMigrationSet {
            from_app_key,
            migration,
        } => {
            // Mirror of `CascadeTargetApplicationSet` but for migration
            // bytes only. ASYMMETRY: this variant does NOT mark contexts
            // `InProgress` or kick the per-context migration propagator —
            // only the paired `CascadeTargetApplicationSet` op kicks
            // contexts into migration. The cascade-emitting RPC handler
            // (PR-2 Task 6) emits both ops in the same governance round
            // when the operator requested a cascade-with-migration, so the
            // status + propagator effects fire exactly once per cascade
            // round (driven by the target-application op, not this one).
            let entries = crate::cascade::walk_for_predicate(store, *group_id, *from_app_key)?;

            // Pre-scan: same atomicity guard as the target-application
            // arm — see the longer rationale comment there.
            for entry in &entries {
                if !entry.matched {
                    continue;
                }
                let entry_permissions = PermissionChecker::new(store, entry.group_id);
                if !entry_permissions.can_manage_application(signer)? {
                    bail!(
                        "cascade group-migration set: signer {} lacks MANAGE_APPLICATION on \
                         descendant {}; aborting before any writes to keep cascade atomic",
                        signer,
                        hex::encode(entry.group_id.to_bytes())
                    );
                }
            }

            let mut any_applied = false;
            for entry in entries {
                if !entry.matched {
                    tracing::debug!(
                        target: "calimero::cascade",
                        group_id = %hex::encode(entry.group_id.to_bytes()),
                        from_app_key = %hex::encode(from_app_key),
                        "CascadeGroupMigrationSet: skip (app_key mismatch)"
                    );
                    continue;
                }
                let entry_settings = GroupSettingsService::new(store, entry.group_id);
                entry_settings.set_group_migration(signer, migration)?;
                any_applied = true;
            }
            if !any_applied {
                tracing::debug!(
                    target: "calimero::cascade",
                    signed_group = %hex::encode(group_id.to_bytes()),
                    from_app_key = %hex::encode(from_app_key),
                    "CascadeGroupMigrationSet: no descendants matched"
                );
            }
            // See the corresponding comment in the
            // `CascadeTargetApplicationSet` arm above — fall through to
            // the shared `Ok((true, divergence))` tail with no
            // divergence report.
            divergence = None;
        }
        #[allow(unreachable_patterns)]
        _ => return Ok((false, None)),
    }

    Ok((true, divergence))
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

    let (handled, _divergence) = apply_group_op_mutations(store, &group_id, &op.signer, &op.op)?;
    // The `_divergence` outcome is dropped on the local-apply path —
    // this entry point is used by callers (local replay, tests) that
    // are not the gossipsub-receive path. The reconcile-via-anchor
    // trigger lives on the namespace-governance receive path
    // (`namespace_governance::apply_signed_op`), which surfaces the
    // report via `NamespaceApplyOutcome::Applied { divergence }`.
    if !handled {
        bail!("unsupported group op variant for local apply");
    }

    let content_hash = op
        .content_hash()
        .map_err(|e| eyre::eyre!("content_hash: {e}"))?;
    let head = get_op_head(store, &group_id)?;
    let next_seq = head.as_ref().map_or(1, |h| h.sequence.saturating_add(1));
    let op_bytes = borsh::to_vec(op).map_err(|e| eyre::eyre!("borsh: {e}"))?;
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
    persist_group_governance_progress(
        store, &group_id, next_seq, &op.signer, op.nonce, new_heads, &op_bytes,
    )?;

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
///
/// `Ok(None)` is a deliberate skip — see
/// [`GroupGovernancePublisher::sign_apply_and_publish`].
pub async fn sign_apply_and_publish(
    store: &Store,
    node_client: &calimero_node_primitives::client::NodeClient,
    ack_router: &calimero_context_client::local_governance::AckRouter,
    group_id: &ContextGroupId,
    signer_sk: &PrivateKey,
    op: GroupOp,
) -> EyreResult<Option<crate::governance_broadcast::DeliveryReport>> {
    GroupGovernancePublisher::new(store, node_client, *group_id)
        .sign_apply_and_publish(ack_router, signer_sk, op)
        .await
}

/// Like [`sign_apply_and_publish`] but attaches a [`KeyRotation`] bundle to
/// the encrypted `MemberRemoved` op. Generates a new group key, wraps it for
/// all remaining members, and stores the new key locally.
///
/// `Ok(None)` is a deliberate skip — see
/// [`GroupGovernancePublisher::sign_apply_and_publish_removal`].
pub async fn sign_apply_and_publish_removal(
    store: &Store,
    node_client: &calimero_node_primitives::client::NodeClient,
    ack_router: &calimero_context_client::local_governance::AckRouter,
    group_id: &ContextGroupId,
    signer_sk: &PrivateKey,
    removed_member: &PublicKey,
) -> EyreResult<Option<crate::governance_broadcast::DeliveryReport>> {
    GroupGovernancePublisher::new(store, node_client, *group_id)
        .sign_apply_and_publish_removal(ack_router, signer_sk, removed_member)
        .await
}

// ---------------------------------------------------------------------------
// Context service name (multi-service bundles)
// ---------------------------------------------------------------------------

/// Store which service from a multi-service bundle a context runs.
/// Called during `ContextRegistered` governance application.
pub fn set_context_service_name(
    store: &Store,
    context_id: &calimero_primitives::context::ContextId,
    service_name: &str,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = calimero_store::key::ContextServiceName::new(*context_id);
    handle.put(
        &key,
        &calimero_store::key::ContextServiceNameValue {
            service_name: service_name.into(),
        },
    )?;
    Ok(())
}

/// Read the service name for a context (if it was created with one).
pub fn get_context_service_name(
    store: &Store,
    context_id: &calimero_primitives::context::ContextId,
) -> EyreResult<Option<String>> {
    let handle = store.handle();
    let key = calimero_store::key::ContextServiceName::new(*context_id);
    Ok(handle.get(&key)?.map(|v| v.service_name.to_string()))
}

#[cfg(test)]
mod test_fixtures;

#[cfg(test)]
mod tests;
