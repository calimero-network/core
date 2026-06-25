//! `calimero-governance-store` — apply pipeline, broadcast/ack flow,
//! metrics, op-events and registration notifications for the local
//! group-governance domain. Extracted from `calimero-context` in
//! #2307 (closes the #2300 epic).
//!
//! The bulk of this file is the original `crates/context/src/
//! group_store/mod.rs` moved here verbatim; helper modules
//! (`governance_broadcast`, `metrics`, `op_events`,
//! `registration_notify`) live alongside it. External callers
//! continue to import via `calimero_context::group_store::*` /
//! `calimero_context::governance_broadcast::*` through curated
//! re-export shims kept in `calimero-context` for backward compat.

use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_primitives::metadata::MetadataRecord;
use calimero_store::key::FromKeyParts;
use calimero_store::key::{
    AsKeyParts, GroupMemberValue, GroupMetaValue, GroupOpHeadValue, GroupUpgradeValue,
};
use calimero_store::types::PredefinedEntry;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use calimero_context_client::local_governance::SignedNamespaceOp;

// Sibling modules — domain-adjacent infrastructure that travels with
// the apply pipeline (event broadcast, metrics, op-events, registration
// signal). `governance_broadcast` is `pub` so external callers can keep
// importing `ObserveDelivery` / `ns_topic` / `verify_readiness_beacon` /
// `sign_ack` through the calimero-context shim.
pub mod governance_broadcast;
pub mod metrics;
pub mod op_events;
pub mod registration_notify;

pub mod absorb;
pub mod absorb_record;
pub mod authorizer;
mod capabilities;
pub mod cascade;
mod context_registration;
mod context_tree;
mod contexts;
mod deny_list;
mod errors;
mod governance_signer;
mod group_governance_publisher;
mod group_keys;
mod group_settings;
mod local_state;
mod membership;
mod meta;
mod metadata;
mod namespace;
pub mod nonce_window;
mod ops;
mod pending_self_purge;
mod permission_checker;
mod signing_keys;
mod tee;
mod upgrade_ladder;
mod upgrades;
use self::local_state::{op_log_contains_content_hash, persist_group_governance_progress};

pub use self::absorb::AbsorbRepository;
pub use self::absorb_record::{AbsorbRecord, AbsorbedEntity, AbsorbedLeaf};
pub use self::authorizer::{
    AtCutAuthorizer, AtCutMembershipPath, LiveFallbackAuthorizer, LIVE_FALLBACK_AUTHORIZER,
};
pub use self::capabilities::CapabilitiesRepository;

pub use self::context_registration::ContextRegistrationService;
pub use self::context_tree::ContextTreeService;
pub use self::contexts::{
    cascade_remove_member_from_group_tree, enumerate_group_contexts, find_local_signing_identity,
    get_group_for_context, is_currently_authorized_for_context, register_context_in_group,
    restore_member_context_identities, unregister_context_from_group,
};
pub use self::deny_list::DenyListRepository;

pub use self::governance_signer::GovernanceSigner;
pub use self::group_governance_publisher::GroupGovernancePublisher;

pub use self::group_keys::{GroupKeyring, StoredGroupKey};
pub use self::group_settings::GroupSettingsService;
pub use self::local_state::{
    delete_group_local_rows, delete_namespace_local_state, get_local_gov_nonce,
    get_member_context_joins, get_op_head, load_nonce_window, read_op_log_after,
    remove_all_member_context_joins, set_local_gov_nonce, store_nonce_window,
    track_member_context_join,
};
pub use self::membership::MembershipRepository;
pub use self::membership::{GroupMembershipView, MembershipPath, MembershipPolicy};
pub use self::meta::MetaRepository;

pub use self::metadata::MetadataRepository;

pub use self::namespace::NamespaceRepository;
pub use self::namespace::MAX_NAMESPACE_DEPTH;
pub use self::namespace::{
    apply_received_group_key, apply_signed_namespace_op, apply_signed_namespace_op_at_cut,
    build_group_key_delivery, collect_skeleton_delta_ids_for_group, decrypt_group_op,
    known_namespace_identities, namespace_groups_awaiting_key,
    namespace_groups_with_held_key_buffered_ops, redrive_buffered_ops_for_group,
    retry_encrypted_ops_for_group, sign_and_publish_namespace_op,
    sign_apply_and_publish_namespace_op, ApplyNamespaceOpResult, CascadePayload, KeyUnwrapFailure,
    NamespaceDagService, NamespaceGovernance, NamespaceHead, NamespaceIdentityRecord,
    NamespaceMembershipService, NamespaceOpLogService, NamespaceRetryService, ReparentOutcome,
    ResolvedNamespaceIdentity,
};
pub use self::pending_self_purge::PendingSelfPurgeRepository;
pub use self::permission_checker::PermissionChecker;
pub use self::signing_keys::SigningKeysRepository;

pub use self::tee::{
    is_quote_hash_used, is_tee_admitted_identity, read_tee_admission_policy, tee_admission_record,
    tee_admission_records, TeeAdmissionPolicy, TeeAdmissionRecord,
};
pub use self::upgrade_ladder::UpgradeLadderRepository;
pub use self::upgrades::UpgradesRepository;

#[cfg(test)]
use self::local_state::{append_op_log_entry, set_op_head};
#[cfg(test)]
use self::upgrades::extract_application_id;

/// A resolved member identity: public key plus its two associated 32-byte keys.
pub type ResolvedIdentity = (PublicKey, [u8; 32], [u8; 32]);

/// The zero-key sentinel written as the placeholder `admin_identity` /
/// `owner_identity` by the bootstrap KeyDelivery seed
/// (`NamespaceGovernance::seed_bootstrap_admin_if_absent`) before the
/// `RootOp::NamespaceCreated` genesis op arrives.
///
/// It grants authority to nobody (it can never equal a real signing
/// identity) and is the single sentinel the genesis anti-hijack gate
/// (`ops::namespace::namespace_created::apply`) checks to distinguish a
/// not-yet-established namespace (placeholder admin/owner) from an
/// established one (real founder). Defined once here so the seed and the
/// gate cannot drift on the magic value (#2474).
pub(crate) const PLACEHOLDER_ADMIN_IDENTITY: [u8; 32] = [0u8; 32];

/// The [`PublicKey`] form of [`PLACEHOLDER_ADMIN_IDENTITY`].
pub(crate) fn placeholder_admin_identity() -> PublicKey {
    PublicKey::from(PLACEHOLDER_ADMIN_IDENTITY)
}

// ---------------------------------------------------------------------------
// Typed errors for group store operations
// ---------------------------------------------------------------------------
//
// Domain-specific error enums live in `errors.rs`. Re-exported below.

pub use self::errors::{
    ApplyError, CapabilitiesError, ContextRegistrationError, GroupCreatedRejection,
    GroupDeletedRejection, KeyringError, MemberJoinedOpenRejection, MembershipError, MetaError,
    NamespaceCreatedRejection, NamespaceError, SigningKeysError,
};

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

// `GroupHandle` is itself a backward-compat facade that predates #2303's
// Repository pattern. Its methods delegate to the (now-deprecated) free
// functions to preserve the existing call surface; new code should use
// the Repositories directly. The `allow(deprecated)` here scopes the
// deprecation warning to "callers of GroupHandle methods", not "inside
// GroupHandle itself" — the latter would just be noise about the facade
// calling its own internals.
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
        MetaRepository::new(self.store).load(&self.group_id)
    }
    pub fn save_meta(&self, meta: &GroupMetaValue) -> EyreResult<()> {
        MetaRepository::new(self.store).save(&self.group_id, meta)
    }
    pub fn delete_meta(&self) -> EyreResult<()> {
        MetaRepository::new(self.store).delete(&self.group_id)
    }
    pub fn compute_state_hash(&self) -> EyreResult<[u8; 32]> {
        MetaRepository::new(self.store).compute_state_hash(&self.group_id)
    }

    // --- Members ---
    pub fn add_member(&self, identity: &PublicKey, role: GroupMemberRole) -> EyreResult<()> {
        MembershipRepository::new(self.store).add_member(&self.group_id, identity, role)
    }
    pub fn add_member_with_keys(
        &self,
        identity: &PublicKey,
        role: GroupMemberRole,
        private_key: Option<[u8; 32]>,
        sender_key: Option<[u8; 32]>,
    ) -> EyreResult<()> {
        MembershipRepository::new(self.store).add_member_with_keys(
            &self.group_id,
            identity,
            role,
            private_key,
            sender_key,
        )
    }
    pub fn remove_member(&self, identity: &PublicKey) -> EyreResult<()> {
        MembershipRepository::new(self.store).remove_member(&self.group_id, identity)
    }
    pub fn list_members(
        &self,
        offset: usize,
        limit: usize,
    ) -> EyreResult<Vec<(PublicKey, GroupMemberRole)>> {
        MembershipRepository::new(self.store).list(&self.group_id, offset, limit)
    }
    pub fn count_members(&self) -> EyreResult<usize> {
        MembershipRepository::new(self.store).count(&self.group_id)
    }
    pub fn count_admins(&self) -> EyreResult<usize> {
        MembershipRepository::new(self.store).count_admins(&self.group_id)
    }
    pub fn is_member(&self, identity: &PublicKey) -> EyreResult<bool> {
        MembershipRepository::new(self.store).is_member(&self.group_id, identity)
    }
    pub fn get_member_value(&self, identity: &PublicKey) -> EyreResult<Option<GroupMemberValue>> {
        MembershipRepository::new(self.store).member_value(&self.group_id, identity)
    }
    pub fn get_member_role(&self, identity: &PublicKey) -> EyreResult<Option<GroupMemberRole>> {
        MembershipRepository::new(self.store).role_of(&self.group_id, identity)
    }

    // --- Authorization ---
    pub fn is_admin(&self, identity: &PublicKey) -> EyreResult<bool> {
        MembershipRepository::new(self.store).is_admin(&self.group_id, identity)
    }
    pub fn is_direct_admin(&self, identity: &PublicKey) -> EyreResult<bool> {
        MembershipRepository::new(self.store).is_direct_admin(&self.group_id, identity)
    }
    pub fn require_admin(&self, identity: &PublicKey) -> EyreResult<()> {
        MembershipRepository::new(self.store).require_admin(&self.group_id, identity)
    }
    pub fn is_admin_or_has_capability(&self, identity: &PublicKey, cap: u32) -> EyreResult<bool> {
        MembershipRepository::new(self.store).is_admin_or_has_capability(
            &self.group_id,
            identity,
            cap,
        )
    }
    pub fn require_admin_or_capability(
        &self,
        identity: &PublicKey,
        cap: u32,
        op: &str,
    ) -> EyreResult<()> {
        MembershipRepository::new(self.store).require_admin_or_capability(
            &self.group_id,
            identity,
            cap,
            op,
        )
    }

    // --- Nonce ---
    pub fn get_local_gov_nonce(&self, signer: &PublicKey) -> EyreResult<Option<u64>> {
        get_local_gov_nonce(self.store, &self.group_id, signer)
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
        SigningKeysRepository::new(self.store).store_key(&self.group_id, pk, sk)
    }
    pub fn get_signing_key(&self, pk: &PublicKey) -> EyreResult<Option<[u8; 32]>> {
        SigningKeysRepository::new(self.store).get_key(&self.group_id, pk)
    }
    pub fn delete_signing_key(&self, pk: &PublicKey) -> EyreResult<()> {
        SigningKeysRepository::new(self.store).delete_key(&self.group_id, pk)
    }
    pub fn require_signing_key(&self, pk: &PublicKey) -> EyreResult<()> {
        SigningKeysRepository::new(self.store).require_key(&self.group_id, pk)
    }

    // --- Group keys ---
    pub fn store_key(&self, group_key: &[u8; 32]) -> EyreResult<[u8; 32]> {
        GroupKeyring::new(self.store, self.group_id).store_key(group_key)
    }
    pub fn load_current_key(&self) -> EyreResult<Option<([u8; 32], [u8; 32])>> {
        GroupKeyring::new(self.store, self.group_id).load_current_key()
    }
    pub fn load_key_by_id(&self, key_id: &[u8; 32]) -> EyreResult<Option<[u8; 32]>> {
        GroupKeyring::new(self.store, self.group_id).load_key_by_id(key_id)
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
        MetadataRepository::new(self.store).count_contexts(&self.group_id)
    }

    // --- Metadata records ---
    pub fn set_metadata(&self, record: &MetadataRecord) -> EyreResult<()> {
        MetadataRepository::new(self.store).set_group(&self.group_id, record)
    }
    pub fn get_metadata(&self) -> EyreResult<Option<MetadataRecord>> {
        MetadataRepository::new(self.store).group_metadata(&self.group_id)
    }
    pub fn set_member_metadata(
        &self,
        member: &PublicKey,
        record: &MetadataRecord,
    ) -> EyreResult<()> {
        MetadataRepository::new(self.store).set_member(&self.group_id, member, record)
    }
    pub fn get_member_metadata(&self, member: &PublicKey) -> EyreResult<Option<MetadataRecord>> {
        MetadataRepository::new(self.store).member_metadata(&self.group_id, member)
    }
    pub fn set_context_metadata(
        &self,
        ctx_id: &ContextId,
        record: &MetadataRecord,
    ) -> EyreResult<()> {
        MetadataRepository::new(self.store).set_context(&self.group_id, ctx_id, record)
    }
    pub fn get_context_metadata(&self, ctx_id: &ContextId) -> EyreResult<Option<MetadataRecord>> {
        MetadataRepository::new(self.store).context_metadata(&self.group_id, ctx_id)
    }
    pub fn enumerate_contexts_with_names(
        &self,
        offset: usize,
        limit: usize,
    ) -> EyreResult<Vec<(ContextId, Option<String>)>> {
        MetadataRepository::new(self.store).enumerate_contexts_with_names(
            &self.group_id,
            offset,
            limit,
        )
    }
    pub fn enumerate_member_metadata(&self) -> EyreResult<Vec<(PublicKey, MetadataRecord)>> {
        MetadataRepository::new(self.store).enumerate_members(&self.group_id)
    }

    // --- Capabilities ---
    pub fn get_member_capability(&self, member: &PublicKey) -> EyreResult<Option<u32>> {
        CapabilitiesRepository::new(self.store).member_capability(&self.group_id, member)
    }
    pub fn set_member_capability(&self, member: &PublicKey, caps: u32) -> EyreResult<()> {
        CapabilitiesRepository::new(self.store).set_member_capability(&self.group_id, member, caps)
    }
    pub fn enumerate_capabilities(&self) -> EyreResult<Vec<(PublicKey, u32)>> {
        CapabilitiesRepository::new(self.store).enumerate_members(&self.group_id)
    }
    pub fn get_default_capabilities(&self) -> EyreResult<Option<u32>> {
        CapabilitiesRepository::new(self.store).default_capabilities(&self.group_id)
    }
    pub fn set_default_capabilities(&self, caps: u32) -> EyreResult<()> {
        CapabilitiesRepository::new(self.store).set_default_capabilities(&self.group_id, caps)
    }
    pub fn get_subgroup_visibility(&self) -> EyreResult<calimero_context_config::VisibilityMode> {
        CapabilitiesRepository::new(self.store).subgroup_visibility(&self.group_id)
    }
    pub fn set_subgroup_visibility(
        &self,
        mode: calimero_context_config::VisibilityMode,
    ) -> EyreResult<()> {
        CapabilitiesRepository::new(self.store).set_subgroup_visibility(&self.group_id, mode)
    }

    // --- Tree ---
    pub fn parent(&self) -> EyreResult<Option<ContextGroupId>> {
        NamespaceRepository::new(self.store).parent(&self.group_id)
    }
    pub fn list_children(&self) -> EyreResult<Vec<ContextGroupId>> {
        NamespaceRepository::new(self.store).list_children(&self.group_id)
    }
    pub fn collect_descendants(&self) -> EyreResult<Vec<ContextGroupId>> {
        NamespaceRepository::new(self.store).collect_descendants(&self.group_id)
    }

    // --- Upgrades ---
    pub fn save_upgrade(&self, upgrade: &GroupUpgradeValue) -> EyreResult<()> {
        UpgradesRepository::new(self.store).save(&self.group_id, upgrade)
    }
    pub fn load_upgrade(&self) -> EyreResult<Option<GroupUpgradeValue>> {
        UpgradesRepository::new(self.store).load(&self.group_id)
    }
    pub fn delete_upgrade(&self) -> EyreResult<()> {
        UpgradesRepository::new(self.store).delete(&self.group_id)
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
        NamespaceRepository::new(self.store).resolve(&self.group_id)
    }
    pub fn resolve_namespace_identity(&self) -> EyreResult<Option<ResolvedIdentity>> {
        NamespaceRepository::new(self.store).resolve_identity(&self.group_id)
    }
    pub fn get_or_create_namespace_identity(
        &self,
    ) -> EyreResult<(ContextGroupId, PublicKey, [u8; 32], [u8; 32])> {
        NamespaceRepository::new(self.store).get_or_create_identity(&self.group_id)
    }

    // --- Per-context capabilities ---
    pub fn set_context_member_capability(
        &self,
        ctx_id: &ContextId,
        member: &PublicKey,
        caps: u8,
    ) -> EyreResult<()> {
        CapabilitiesRepository::new(self.store).set_context_member(
            &self.group_id,
            ctx_id,
            member,
            caps,
        )
    }
    pub fn get_context_member_capability(
        &self,
        ctx_id: &ContextId,
        member: &PublicKey,
    ) -> EyreResult<Option<u8>> {
        CapabilitiesRepository::new(self.store).context_member_capability(
            &self.group_id,
            ctx_id,
            member,
        )
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

    pub fn get_identity(&self) -> EyreResult<Option<ResolvedIdentity>> {
        NamespaceRepository::new(self.store).identity(&ContextGroupId::from(self.namespace_id))
    }

    pub fn store_identity(
        &self,
        pk: &PublicKey,
        sk: &[u8; 32],
        sender: &[u8; 32],
    ) -> EyreResult<()> {
        NamespaceRepository::new(self.store).store_identity(
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
        MetaRepository::new(self.store).enumerate_all(offset, limit)
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
        UpgradesRepository::new(self.store).enumerate_in_progress()
    }

    pub fn resolve_namespace(&self, group_id: &ContextGroupId) -> EyreResult<ContextGroupId> {
        NamespaceRepository::new(self.store).resolve(group_id)
    }

    pub fn is_read_only_for_context(
        &self,
        context_id: &ContextId,
        identity: &PublicKey,
    ) -> EyreResult<bool> {
        NamespaceRepository::new(self.store).is_read_only_for_context(context_id, identity)
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

/// Wall-clock seconds since the Unix epoch. Single source for the local
/// invitation-expiry fast-fail, the responder key-delivery gate, and the
/// joiner-stamped `joined_at` on `MemberJoinedAt`. Mirrors [`now_millis`];
/// a clock-failure yields 0, which is safe because the authoritative
/// expiry enforcement is the deterministic apply gate plus the responder
/// gate (each reading its own clock) — the fast-fail is advisory only.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
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
///
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

pub(crate) fn verify_post_apply_state_hashes(
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
        match MetaRepository::new(store).compute_state_hash(group_id) {
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
        match MetaRepository::new(store).snapshot_context_state_hashes(group_id) {
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
/// Read failures are downgraded to a warn log and `Ok(None)` — the
/// apply-site has already written the row and committed by the time
/// this is called, so a transient read failure here should not roll
/// back the op via the caller's `?`. The synthesized event is a
/// best-effort optimisation; missing it means the joiner won't
/// backfill pre-existing contexts in this group, but they'll still
/// auto-follow future ones via the `ContextRegistered` event handler.
pub(crate) fn build_auto_follow_set_if_enabled(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
) -> EyreResult<Option<crate::op_events::OpEvent>> {
    let value = match MembershipRepository::new(store).member_value(group_id, member) {
        Ok(Some(v)) => v,
        Ok(None) => {
            tracing::warn!(
                group_id = %hex::encode(group_id.to_bytes()),
                %member,
                "post-apply read found no member row — skipping auto-follow emission"
            );
            return Ok(None);
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
            return Ok(None);
        }
    };
    if value.auto_follow.contexts {
        Ok(Some(crate::op_events::OpEvent::AutoFollowSet {
            group_id: group_id.to_bytes(),
            member: *member,
            contexts: true,
            subgroups: value.auto_follow.subgroups,
        }))
    } else {
        Ok(None)
    }
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
    parents: &[[u8; 32]],
    authorizer: &dyn crate::authorizer::AtCutAuthorizer,
) -> EyreResult<(
    bool,
    Option<DivergenceReport>,
    Vec<crate::op_events::OpEvent>,
)> {
    // The op's causal cut + at-cut authorizer ride into the apply gates so the
    // `PermissionChecker` admin/capability gates resolve against the projection at
    // the op's cut, with live as the `None`-fallback (F5 #28 stage 4). The default
    // (live-fallback) authorizer keeps callers without an apply-auth context on live.
    let mut ctx = ops::group::GroupApplyCtx::new_with_apply_auth(
        store, group_id, signer, parents, authorizer,
    );
    let handled = ops::group::dispatch(&mut ctx, op)?;
    Ok((handled, ctx.divergence, ctx.pending_events))
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
    apply_local_signed_group_op_at_cut(store, op, &crate::authorizer::LIVE_FALLBACK_AUTHORIZER)
}

/// [`apply_local_signed_group_op`] with the at-cut apply-auth source attached: the
/// `PermissionChecker` admin/capability gates resolve against the projection at the
/// op's own `parent_op_hashes`, with live as the `None`-fallback (F5 #28 stage 4).
/// The production apply path (the group DAG applier) injects a projection-backed
/// authorizer; the plain `apply_local_signed_group_op` passes the inert live
/// fallback so existing callers (tests, replay) are unchanged.
pub fn apply_local_signed_group_op_at_cut(
    store: &Store,
    op: &SignedGroupOp,
    authorizer: &dyn crate::authorizer::AtCutAuthorizer,
) -> EyreResult<()> {
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
        // Mirror the namespace receive-side bypass (apply_group_op_inner): if
        // the subgroup meta row hasn't been written yet — e.g. a group op
        // buffered/replayed before its GroupCreated lands — there is no state
        // to hash. `compute_state_hash` would raise GroupNotFoundForHash and
        // strand the op forever (#2848). Treat absent meta as a bypass;
        // signature and nonce checks remain in force.
        let repo = MetaRepository::new(store);
        if repo.load(&group_id)?.is_some() {
            let current_state_hash = repo.compute_state_hash(&group_id)?;
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
    }

    let mut nonce_window = load_nonce_window(store, &group_id, &op.signer)?;
    if nonce_window.contains(op.nonce) {
        tracing::debug!(
            nonce = op.nonce,
            floor = nonce_window.floor(),
            signer = %op.signer,
            "ignoring op with already-processed nonce"
        );
        return Ok(());
    }

    let (handled, _divergence, pending_events) = apply_group_op_mutations(
        store,
        &group_id,
        &op.signer,
        &op.op,
        &op.parent_op_hashes,
        authorizer,
    )?;
    // The `_divergence` outcome is dropped on the local-apply path —
    // this entry point is used by callers (local replay, tests) that
    // are not the gossipsub-receive path. The reconcile-via-anchor
    // trigger lives on the namespace-governance receive path
    // (`namespace_governance::apply_signed_op`), which surfaces the
    // report via `NamespaceApplyOutcome::Applied { divergence }`.
    if !handled {
        bail!(ApplyError::UnsupportedOp);
    }

    let content_hash = op
        .content_hash()
        .map_err(|e| eyre::eyre!("content_hash: {e}"))?;

    // Record AFTER the apply succeeded above. `record` advances the floor
    // through any now-contiguous run; for a sibling that arrived out of
    // order (the #2516 case) it just adds to the above-floor set so the
    // lower-nonce sibling still applies when it lands.
    nonce_window.record(op.nonce);

    // Idempotent op-log append, mirroring the namespace receive path
    // (`apply_group_op_inner`). The nonce guard above short-circuits the
    // common replay, but `store_nonce_window`'s two-key write is not atomic:
    // a crash between the floor and above-set writes can drop an in-flight
    // above-floor nonce from the persisted window, so a DAG replay (this
    // function runs under `governance_dag`) can reach here for an op that is
    // already in the log. Dedup against the PERSISTED op-log — the monotonic
    // signal used everywhere else — so the re-apply re-persists the advanced
    // window WITHOUT appending a duplicate entry.
    //
    // REPLAY-SAFETY CONTRACT: the mutation above (`apply_group_op_mutations`)
    // re-runs on this replay, BEFORE this dedup fires. That is safe because
    // every group-op handler is idempotent on re-apply — e.g. `MemberAdded`
    // resolves to `MembershipRepository::add_member`, an upsert (`put`) that
    // succeeds whether or not the member row already exists and never errors
    // on a duplicate. This is the same contract the namespace receive path and
    // `retry_encrypted_ops_for_group` already depend on (both re-feed applied
    // ops through the mutation path). A handler that instead errored on a
    // duplicate would leave the window un-persisted and the node stuck
    // retrying — so idempotency is a hard requirement for any new handler.
    if op_log_contains_content_hash(store, &group_id, &content_hash)? {
        store_nonce_window(store, &group_id, &op.signer, &nonce_window)?;
        // #2770 — CANONICAL note on the dropped-on-dedup tradeoff (the
        // namespace group-op and RootOp dedup branches share it):
        //
        // `pending_events` is intentionally dropped here. The mutation above
        // re-collected events on this replay, but the op is already logged, so
        // re-emitting would fire the event again on EVERY ordinary duplicate
        // (network re-gossip, DAG replay) — exactly the duplicate-notification
        // behaviour this change removes. Subscribers are idempotent + lossy-
        // tolerant, so no-re-emit is strictly more correct for the common case.
        //
        // Accepted edge: a crash landing BETWEEN the op-log append below and
        // the post-append flush leaves the op logged but its events un-emitted;
        // the restart replay reaches this branch and drops them, so a one-time
        // signal (e.g. `TeeMemberRemoved`) is not redelivered. This is the SAME
        // bounded gap already documented + accepted for the broadcast
        // lagged-drop case (see `calimero-context::self_purge` run-loop `Lagged`
        // arm): NOT a forward-secrecy hole — FS on future writes is held by the
        // key-rotation pipeline, not this purge — the residue is only stale,
        // already-orphaned local key material.
        //
        // We deliberately do NOT re-emit here to cover that window: this branch
        // cannot distinguish a crash-recovery replay from an ordinary duplicate,
        // so it would regress the common case. A crash-safe fix belongs on the
        // apply path (writing the self-purge marker deterministically as every
        // node applies), tracked as TEE-lifecycle follow-up (#2772/#2771).
        return Ok(());
    }

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
        store,
        &group_id,
        next_seq,
        &op.signer,
        &nonce_window,
        new_heads,
        &op_bytes,
    )?;

    // #2770: flush events only after the op-log entry is durably appended.
    for event in pending_events {
        crate::op_events::notify(event);
    }
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
    // Mint above the highest applied nonce, not just the contiguous floor:
    // if this signer already has out-of-order siblings in its window
    // (`above` non-empty), `max_applied` is higher than `floor`, so the next
    // authored op gets a fresh nonce instead of colliding with one already in
    // the gap.
    let last = load_nonce_window(store, group_id, &signer_sk.public_key())?.max_applied();
    let nonce = last.checked_add(1).ok_or(ApplyError::NonceOverflow)?;
    let parent_hashes = get_op_head(store, group_id)?
        .map(|h| h.dag_heads.clone())
        .unwrap_or_default();
    let state_hash = MetaRepository::new(store).compute_state_hash(group_id)?;
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
