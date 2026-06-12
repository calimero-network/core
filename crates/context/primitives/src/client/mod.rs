#![allow(clippy::multiple_inherent_impl, reason = "better readability")]

use std::sync::Arc;

use async_stream::try_stream;
use borsh::BorshDeserialize;
use calimero_context_config::types::{ContextGroupId, InvitationFromMember, SignedOpenInvitation};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::alias::Alias;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::common::DIGEST_SIZE;
use calimero_primitives::context::{Context, ContextId};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_primitives::metadata::MetadataRecord;
use calimero_store::slice::Slice;
use calimero_store::tx::Transaction;
use calimero_store::{key, types, Store};
use calimero_utils_actix::LazyRecipient;
use eyre::{ContextCompat, WrapErr};
use futures_util::Stream;
use rand::Rng;
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;

use crate::group::{
    AbortMigrationRequest, AbortMigrationResponse, AddGroupMembersRequest, AdmitTeeNodeRequest,
    BroadcastGroupLocalStateRequest, CascadeStatusEntry, CreateGroupInvitationRequest,
    CreateGroupInvitationResponse, CreateGroupRequest, CreateGroupResponse, DeleteGroupRequest,
    DeleteGroupResponse, DeleteNamespaceRequest, DeleteNamespaceResponse,
    DetachContextFromGroupRequest, GetCascadeStatusRequest, GetContextMetadataRequest,
    GetGroupForContextRequest, GetGroupInfoRequest, GetGroupMetadataRequest,
    GetGroupUpgradeStatusRequest, GetMemberCapabilitiesRequest, GetMemberCapabilitiesResponse,
    GetMemberMetadataRequest, GetMigrationStatusRequest, GetNamespaceIdentityRequest,
    GroupContextEntry, GroupInfoResponse, GroupSummary, GroupUpgradeInfo,
    IssueNamespaceOwnershipProofRequest, IssueOwnershipProofRequest, IssueOwnershipProofResponse,
    JoinContextRequest, JoinContextResponse, JoinGroupRequest, JoinGroupResponse,
    JoinSubgroupInheritanceRequest, JoinSubgroupInheritanceResponse, LeaveContextRequest,
    LeaveContextResponse, LeaveGroupRequest, LeaveGroupResponse, LeaveNamespaceRequest,
    LeaveNamespaceResponse, ListAllGroupsRequest, ListGroupContextsRequest,
    ListGroupMembersRequest, ListGroupMembersResponse, ListNamespacesForApplicationRequest,
    ListNamespacesRequest, MigrationStatus, NamespaceSummary, RemoveGroupMembersRequest,
    RetryGroupUpgradeRequest, SetContextMetadataRequest, SetDefaultCapabilitiesRequest,
    SetGroupMetadataRequest, SetMemberAutoFollowRequest, SetMemberCapabilitiesRequest,
    SetMemberMetadataRequest, SetSubgroupVisibilityRequest, SetTeeAdmissionPolicyRequest,
    StoreContextMetadataRequest, StoreDefaultCapabilitiesRequest, StoreGroupContextRequest,
    StoreGroupMetaRequest, StoreGroupMetadataRequest, StoreMemberCapabilityRequest,
    StoreMemberMetadataRequest, StoreSubgroupVisibilityRequest, SyncGroupRequest,
    SyncGroupResponse, UpdateGroupSettingsRequest, UpdateMemberRoleRequest, UpgradeGroupRequest,
    UpgradeGroupResponse,
};
use crate::local_governance::AckRouter;
use crate::messages::{
    AcquireContextLockRequest, ApplySignedGroupOpRequest, ApplySignedNamespaceOpRequest,
    ContextMessage, CreateContextRequest, CreateContextResponse, DeleteContextRequest,
    DeleteContextResponse, ExecuteError, ExecuteRequest, ExecuteResponse, MigrationParams,
    NamespaceApplyOutcome, UpdateApplicationRequest,
};
use crate::{ContextAtomic, ContextAtomicKey};

mod context_api;
pub mod crypto;
mod sync;

/// A registry of context metadata backed by a key-value store.
///
/// Provides synchronous read/write access to context metadata, DAG heads,
/// root hashes, and membership information without requiring the actor mailbox.
#[derive(Clone, Debug)]
pub struct ContextRegistry {
    datastore: Store,
}

/// One row of [`ContextRegistry::dump_root`] — a child entry in the
/// context root's Merkle index. Diagnostic-only: a #2319 flake ("same
/// DAG heads, different root hash") is triaged by diffing the two
/// peers' dumps. Field names are stable for `tracing` consumption
/// (json output keys).
///
/// Not stable API — pulled in by `calimero-node`'s heartbeat handler
/// across crate boundaries, which is why it's `pub` rather than
/// `pub(crate)`.
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct RootChildDump {
    pub id: [u8; 32],
    pub merkle_hash: [u8; 32],
    pub created_at: u64,
    pub updated_at: u64,
    pub crdt_type: Option<calimero_primitives::crdt::CrdtType>,
    pub field_name: Option<String>,
}

/// Companion to [`RootChildDump`] — captures ROOT's own_hash + full_hash
/// + Key::Entry(ROOT) bytes hash. Lets a #2319 diff distinguish
/// "children diverge" from "ROOT.own_hash diverges with identical
/// children" (the latter pattern surfaced on PR #2472 attempt 1).
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct RootSelfDump {
    pub own_hash: [u8; 32],
    pub full_hash: [u8; 32],
    /// Sha256 of `Key::Entry(ROOT)` if it exists. `None` if no entry.
    pub entry_bytes_hash: Option<[u8; 32]>,
    pub entry_bytes_len: usize,
    pub children_count: usize,
}

/// Borsh layout-faithful mirrors of `calimero_storage::index::EntityIndex`
/// and the embedded `entities::ChildInfo`/`Metadata`/`StorageType`/
/// `SignatureData` types. Used by [`ContextRegistry::compute_root_hash`]
/// and [`ContextRegistry::dump_root`] to decode the index without pulling
/// in the full `calimero-storage` types (which would force a dep cycle).
///
/// **SYNC NOTE**: When `calimero_storage::index::EntityIndex` or its
/// transitively-borshed children change, update these mirrors *and*
/// `calimero-storage/src/tests/index.rs::minimal_struct_layout_compat`.
/// A missed update produces silent misdeserialization on a diagnostic
/// path that fires only during rare divergence events — exactly when
/// correct output matters most.
mod borsh_layout {
    use borsh::BorshDeserialize;
    use calimero_primitives::crdt::CrdtType;

    /// Captures every field [`super::ContextRegistry::dump_root`] needs in
    /// a single pass: children (with metadata), full_hash, own_hash.
    /// `compute_root_hash_via_borsh` reads only `full_hash`.
    #[derive(BorshDeserialize)]
    #[allow(dead_code, reason = "fields required for borsh layout fidelity")]
    pub(super) struct EntityIndex {
        pub(super) id: [u8; 32],
        pub(super) parent_id: Option<[u8; 32]>,
        pub(super) children: Option<Vec<ChildInfo>>,
        pub(super) full_hash: [u8; 32],
        pub(super) own_hash: [u8; 32],
    }

    #[derive(BorshDeserialize)]
    #[allow(dead_code, reason = "fields required for borsh layout fidelity")]
    pub(super) struct ChildInfo {
        pub(super) id: [u8; 32],
        pub(super) merkle_hash: [u8; 32],
        pub(super) metadata: Metadata,
    }

    #[derive(BorshDeserialize)]
    #[allow(dead_code, reason = "fields required for borsh layout fidelity")]
    pub(super) struct Metadata {
        pub(super) created_at: u64,
        pub(super) updated_at: u64,
        pub(super) storage_type: StorageType,
        pub(super) crdt_type: Option<CrdtType>,
        pub(super) field_name: Option<String>,
        pub(super) schema_version: Option<u32>,
    }

    #[derive(BorshDeserialize)]
    #[allow(dead_code, reason = "borsh layout-faithful enum mirror")]
    pub(super) enum StorageType {
        Public,
        User {
            owner: [u8; 32],
            signature_data: Option<SignatureData>,
        },
        Frozen,
        Shared {
            // Real type: BTreeMap<PublicKey, OpMask> (#2738). PublicKey is
            // [u8;32], OpMask is a u8 newtype, so the borsh layout is a map of
            // 32-byte key → 1-byte value. The old mirror had BTreeSet<[u8;32]>
            // (no per-writer OpMask byte), which under-counted each writer by one
            // byte and misaligned the rest of the EntityIndex — surfacing as
            // "Invalid Option representation" in compute_root_hash_via_borsh once
            // a Shared entity appears among the root's children.
            writers: std::collections::BTreeMap<[u8; 32], u8>,
            signature_data: Option<SignatureData>,
        },
        // Real type carries this variant (every SharedStorage member entity);
        // its absence made the mirror unable to decode any root whose children
        // include a member — the cold-join failure mode.
        SharedMember {
            anchor: [u8; 32],
            signature_data: Option<SignatureData>,
        },
    }

    #[derive(BorshDeserialize)]
    #[allow(dead_code, reason = "fields required for borsh layout fidelity")]
    pub(super) struct SignatureData {
        pub(super) signature: [u8; 64],
        pub(super) nonce: u64,
        pub(super) signer: Option<[u8; 32]>,
    }
}

impl ContextRegistry {
    #[must_use]
    pub const fn new(datastore: Store) -> Self {
        Self { datastore }
    }

    /// Returns a handle to the datastore for direct access.
    /// Used by node components that need to read stored data.
    pub fn datastore_handle(&self) -> calimero_store::Handle<Store> {
        self.datastore.handle()
    }

    /// Returns a reference to the underlying `Store`.
    /// Used by governance operations that need direct store access.
    pub fn datastore(&self) -> &Store {
        &self.datastore
    }

    /// Checks if a context's metadata exists in the local datastore.
    ///
    /// # Arguments
    ///
    /// * `context_id` - The ID of the context to check for.
    ///
    /// # Returns
    ///
    /// A `Result` containing `true` if the context exists locally, `false` otherwise.
    pub fn has_context(&self, context_id: &ContextId) -> eyre::Result<bool> {
        let handle = self.datastore.handle();

        let key = key::ContextMeta::new(*context_id);

        Ok(handle.has(&key)?)
    }

    /// Retrieves a context metadata from the local datastore.
    ///
    /// # Arguments
    ///
    /// * `context_id` - The ID of the context to retrieve.
    ///
    /// # Returns
    ///
    /// A `Result` containing `Some(Context)` if the context is found, or `None` if it is not.
    pub fn get_context(&self, context_id: &ContextId) -> eyre::Result<Option<Context>> {
        let handle = self.datastore.handle();

        let key = key::ContextMeta::new(*context_id);

        let Some(meta) = handle.get(&key)? else {
            return Ok(None);
        };

        // Resolve the application's semver from its ApplicationMeta row so the
        // Context response carries it directly (skew #2 — bundle version). The
        // row may be absent (e.g. uninstalled app); leave the version None then.
        let application_version = handle
            .get(&meta.application)?
            .map(|app| app.version.to_string());

        // Human-readable name from the owning group's per-context metadata
        // record, when set.
        let name = handle
            .get(&key::ContextGroupRef::new((*context_id).into()))?
            .and_then(|gid: [u8; 32]| {
                handle
                    .get(&key::GroupContextMetadata::new(gid, (*context_id).into()))
                    .ok()
                    .flatten()
            })
            .and_then(|record| record.name);

        let context = Context::with_service(
            *context_id,
            meta.application.application_id(),
            meta.root_hash.into(),
            meta.dag_heads.clone(),
            meta.service_name.as_deref().map(String::from),
        )
        .with_application_version(application_version)
        .with_name(name);

        tracing::debug!(
            %context_id,
            dag_heads_count = meta.dag_heads.len(),
            "Loaded context from database"
        );

        Ok(Some(context))
    }

    /// Updates the DAG heads for a context after applying a delta.
    ///
    /// # Arguments
    ///
    /// * `context_id` - The ID of the context to update.
    /// * `dag_heads` - The new DAG heads (typically the delta ID that was just applied).
    ///
    /// # Returns
    ///
    /// A `Result` indicating success or failure.
    pub fn update_dag_heads(
        &self,
        context_id: &ContextId,
        dag_heads: Vec<[u8; 32]>,
    ) -> eyre::Result<()> {
        let handle = self.datastore.handle();

        let key = key::ContextMeta::new(*context_id);

        let Some(mut meta) = handle.get(&key)? else {
            eyre::bail!("Context not found: {}", context_id);
        };

        // Update dag_heads
        meta.dag_heads = dag_heads.clone();

        // Write back to database
        self.datastore.clone().handle().put(&key, &meta)?;

        tracing::debug!(
            %context_id,
            dag_heads_count = dag_heads.len(),
            "Updated dag_heads in database"
        );

        Ok(())
    }

    /// Atomically persist a batch of applied DAG-delta records together with
    /// the context's updated `dag_heads`, in a single backend write batch.
    ///
    /// Either every delta record *and* the new `dag_heads` land, or none do.
    /// A per-record `put` loop followed by a separate [`update_dag_heads`]
    /// could be interrupted partway — a serialization slip, an I/O error, a
    /// crash — leaving some cascaded deltas persisted as `applied: true`
    /// while `dag_heads` stayed stale. On restart, the delta-load path would
    /// miss the unpersisted deltas and the in-memory DAG and the DB could
    /// diverge permanently for that context. Folding the whole set into one
    /// [`Store::apply`] (a RocksDB `WriteBatch`) closes that window: a
    /// failure leaves the pre-cascade state untouched, so the next sync
    /// replays cleanly.
    ///
    /// `deltas` pairs each delta's storage key with the record to write. An
    /// empty `deltas` slice is valid: the batch then carries only the
    /// `dag_heads` update (still atomic), which is how a cascade with no
    /// newly-applied deltas advances heads — the behaviour the standalone
    /// [`update_dag_heads`] used to provide.
    ///
    /// Atomicity covers the *write* only. The `meta` read, the in-memory
    /// `dag_heads` mutation, and the commit are not one transaction, so two
    /// callers racing on the same context could read the same `meta` and the
    /// later commit would clobber the earlier one's heads (a read-modify-write
    /// race, same as the standalone [`update_dag_heads`] it replaces). The
    /// node's callers serialise per-context behind the DAG write lock, so
    /// this is not hit in practice; a CAS / per-context lock would be needed
    /// to make it safe for unsynchronised callers.
    ///
    /// [`update_dag_heads`]: Self::update_dag_heads
    pub fn persist_deltas_and_dag_heads(
        &self,
        context_id: &ContextId,
        deltas: &[(key::ContextDagDelta, types::ContextDagDelta)],
        dag_heads: Vec<[u8; 32]>,
    ) -> eyre::Result<()> {
        let meta_key = key::ContextMeta::new(*context_id);

        let Some(mut meta) = self.datastore.handle().get(&meta_key)? else {
            eyre::bail!("Context not found: {}", context_id);
        };
        meta.dag_heads = dag_heads;
        let dag_heads_count = meta.dag_heads.len();

        // Stage every put into one transaction, then commit it as a single
        // atomic write. Values are encoded into owned byte slices as we go;
        // a serialization failure aborts here, before `apply`, so nothing is
        // written. `ContextDagDelta` lives in the `Delta` column and
        // `ContextMeta` in `Meta`; a RocksDB `WriteBatch` is atomic across
        // column families, so the heads update can't land without the deltas
        // (or vice versa).
        let mut tx = Transaction::default();
        for (delta_key, record) in deltas {
            let value: Slice<'_> = borsh::to_vec(record)?.into();
            tx.put(delta_key, value);
        }
        let meta_bytes: Slice<'_> = borsh::to_vec(&meta)?.into();
        tx.put(&meta_key, meta_bytes);

        self.datastore.apply(&tx)?;

        tracing::debug!(
            %context_id,
            delta_count = deltas.len(),
            dag_heads_count,
            "Atomically persisted cascaded deltas and dag_heads"
        );

        Ok(())
    }

    /// Atomically persist a batch of DAG-delta records in a single backend
    /// write, *without* touching `dag_heads`.
    ///
    /// This is the sibling of [`persist_deltas_and_dag_heads`] for callers
    /// that manage heads themselves (e.g. snapshot-boundary checkpoints,
    /// whose heads are derived from the snapshot transfer). The batch is
    /// all-or-nothing — a crash or I/O error can't leave a subset of the
    /// records on disk while the rest are missing. An empty slice is a no-op.
    ///
    /// [`persist_deltas_and_dag_heads`]: Self::persist_deltas_and_dag_heads
    pub fn persist_delta_records(
        &self,
        deltas: &[(key::ContextDagDelta, types::ContextDagDelta)],
    ) -> eyre::Result<()> {
        if deltas.is_empty() {
            return Ok(());
        }

        let mut tx = Transaction::default();
        for (delta_key, record) in deltas {
            let value: Slice<'_> = borsh::to_vec(record)?.into();
            tx.put(delta_key, value);
        }

        self.datastore.apply(&tx)?;

        tracing::debug!(
            delta_count = deltas.len(),
            "Atomically persisted delta records"
        );

        Ok(())
    }

    /// Atomically deletes a batch of DAG delta rows by key (issue #2026
    /// compaction). Like [`persist_delta_records`](Self::persist_delta_records),
    /// the whole batch lands in one [`Transaction`] so a crash mid-prune can
    /// never leave the delta column half-deleted. Pruning is idempotent:
    /// deleting an already-absent key is a no-op.
    pub fn prune_delta_records(&self, delta_keys: &[key::ContextDagDelta]) -> eyre::Result<()> {
        if delta_keys.is_empty() {
            return Ok(());
        }

        let mut tx = Transaction::default();
        for delta_key in delta_keys {
            tx.delete(delta_key);
        }

        self.datastore.apply(&tx)?;

        tracing::debug!(
            delta_count = delta_keys.len(),
            "Atomically pruned delta records"
        );

        Ok(())
    }

    /// Updates the ApplicationId for a context.
    ///
    /// # Arguments
    ///
    /// * `context_id` - The ID of the context to update.
    /// * `application_id` - The new ApplicationId.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success or failure.
    pub fn update_context_application_id(
        &self,
        context_id: &ContextId,
        application_id: ApplicationId,
    ) -> eyre::Result<()> {
        let handle = self.datastore.handle();

        let key = key::ContextMeta::new(*context_id);

        let Some(mut meta) = handle.get(&key)? else {
            eyre::bail!("Context not found: {}", context_id);
        };

        // Update application_id
        meta.application = key::ApplicationMeta::new(application_id);

        // Write back to database
        self.datastore.clone().handle().put(&key, &meta)?;

        tracing::debug!(
            %context_id,
            %application_id,
            "Updated application_id in database"
        );

        Ok(())
    }

    /// Computes the actual root hash from storage by reading the root Index entry.
    ///
    /// This reads the EntityIndex for Id::root() from RocksDB and extracts the
    /// Merkle full_hash. This is the authoritative hash computed from the actual
    /// state, not a claimed value.
    ///
    /// # Arguments
    ///
    /// * `context_id` - The ID of the context to compute the root hash for.
    ///
    /// # Returns
    ///
    /// The computed root hash, or `[0; 32]` if no root index exists (empty state).
    /// State-key bytes for `Key::Index(Id::root())` of a context.
    /// `Key::Index(id).to_bytes() = SHA256([0] || id.as_bytes())`.
    fn index_state_key(context_id: &ContextId) -> [u8; 32] {
        let mut key_bytes = [0u8; 33];
        key_bytes[0] = 0; // Index discriminant
        key_bytes[1..33].copy_from_slice(&**context_id);
        Sha256::digest(key_bytes).into()
    }

    /// State-key bytes for `Key::Entry(Id::root())` of a context.
    /// `Key::Entry(id).to_bytes() = SHA256([1] || id.as_bytes())`.
    fn entry_state_key(context_id: &ContextId) -> [u8; 32] {
        let mut key_bytes = [0u8; 33];
        key_bytes[0] = 1; // Entry discriminant
        key_bytes[1..33].copy_from_slice(&**context_id);
        Sha256::digest(key_bytes).into()
    }

    pub fn compute_root_hash(&self, context_id: &ContextId) -> eyre::Result<[u8; 32]> {
        let state_key = Self::index_state_key(context_id);
        let handle = self.datastore.handle();
        let db_key = key::ContextState::new(*context_id, state_key);

        // Get data and convert to owned bytes to avoid lifetime issues
        let data_opt = handle.get(&db_key)?;

        match data_opt {
            Some(data) => {
                // Convert to owned Vec<u8> to avoid lifetime issues
                let bytes: Vec<u8> = data.as_ref().to_vec();
                drop(data); // Explicitly drop the borrowed data

                self.parse_entity_index_root_hash(context_id, &bytes)
            }
            None => {
                // No root index exists - empty state
                tracing::debug!(
                    %context_id,
                    "No root index found, returning zero hash"
                );
                Ok([0; 32])
            }
        }
    }

    /// Parse EntityIndex bytes to extract the root hash.
    fn parse_entity_index_root_hash(
        &self,
        context_id: &ContextId,
        bytes: &[u8],
    ) -> eyre::Result<[u8; 32]> {
        // Deserialize EntityIndex and extract full_hash
        // EntityIndex is borsh-serialized with full_hash at a known offset
        // Structure: id(32) + parent_id(Option<32>) + children(Option<Vec>) + full_hash(32) + ...

        if bytes.len() < 68 {
            // Minimum size: id(32) + parent_id_tag(1) + children_tag(1) + full_hash(32) + own_hash(32) = 98
            // But we check for 68 to be safe (id + tags + full_hash)
            eyre::bail!(
                "EntityIndex too small: {} bytes, expected at least 68",
                bytes.len()
            );
        }

        // Parse the EntityIndex structure manually for efficiency
        // id: [u8; 32]
        // parent_id: Option<Id> - 1 byte tag + optional 32 bytes
        // children: Option<Vec<ChildInfo>> - 1 byte tag + optional length + data
        // full_hash: [u8; 32]

        let mut offset = 32; // Skip id

        // Skip parent_id (Option<Id>)
        let parent_tag = bytes[offset];
        offset += 1;
        if parent_tag == 1 {
            offset += 32; // Skip the Id bytes
        }

        // Skip children (Option<Vec<ChildInfo>>)
        let children_tag = bytes[offset];
        offset += 1;
        if children_tag == 1 {
            // Children present - use full borsh deserialization for correctness
            return self.compute_root_hash_via_borsh(context_id, bytes);
        }

        // Now at full_hash position
        if offset + 32 > bytes.len() {
            eyre::bail!(
                "EntityIndex full_hash truncated at offset {}, len {}",
                offset,
                bytes.len()
            );
        }

        let mut full_hash = [0u8; 32];
        full_hash.copy_from_slice(&bytes[offset..offset + 32]);

        tracing::debug!(
            %context_id,
            computed_root = ?Hash::from(full_hash),
            "Computed root hash from storage"
        );

        Ok(full_hash)
    }

    /// Helper to compute root hash using full borsh deserialization.
    ///
    /// Used when `parse_entity_index_root_hash`'s fast-path can't skip past
    /// the `children` field (i.e. children present). Reuses the shared
    /// [`borsh_layout::EntityIndex`] mirror so that any layout change
    /// touches exactly one definition.
    fn compute_root_hash_via_borsh(
        &self,
        context_id: &ContextId,
        bytes: &[u8],
    ) -> eyre::Result<[u8; 32]> {
        let mut reader: &[u8] = bytes;
        let index = borsh_layout::EntityIndex::deserialize_reader(&mut reader).map_err(|e| {
            tracing::warn!(
                %context_id,
                error = %e,
                total_bytes = bytes.len(),
                "EntityIndex borsh decode failed in compute_root_hash_via_borsh"
            );
            eyre::eyre!("Failed to deserialize EntityIndex: {}", e)
        })?;

        let trailing = reader.len();
        if trailing > 0 {
            tracing::debug!(
                %context_id,
                trailing_bytes = trailing,
                total_bytes = bytes.len(),
                "EntityIndex deserialization skipped trailing bytes"
            );
        }

        tracing::debug!(
            %context_id,
            computed_root = ?Hash::from(index.full_hash),
            "Computed root hash from storage (via borsh)"
        );

        Ok(index.full_hash)
    }

    /// Forces the root hash for a context to a specific value.
    ///
    /// **WARNING**: This bypasses verification and should only be used when
    /// the hash has already been verified or during controlled operations.
    /// Prefer `compute_root_hash` + `set_root_hash` for safety.
    ///
    /// # Arguments
    ///
    /// * `context_id` - The ID of the context to update.
    /// * `root_hash` - The root hash to set.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success or failure.
    pub fn force_root_hash(&self, context_id: &ContextId, root_hash: Hash) -> eyre::Result<()> {
        let handle = self.datastore.handle();

        let key = key::ContextMeta::new(*context_id);

        let Some(mut meta) = handle.get(&key)? else {
            eyre::bail!("Context not found: {}", context_id);
        };

        tracing::debug!(
            %context_id,
            old_root = ?Hash::from(meta.root_hash),
            new_root = ?root_hash,
            "Setting root hash"
        );

        meta.root_hash = *root_hash;

        self.datastore.clone().handle().put(&key, &meta)?;

        Ok(())
    }

    /// Verifies that the stored root hash matches the actual state.
    ///
    /// Computes the root hash from storage and compares with the claimed hash.
    /// Returns Ok(()) if they match, or an error describing the mismatch.
    ///
    /// # Arguments
    ///
    /// * `context_id` - The ID of the context to verify.
    /// * `claimed_hash` - The hash to verify against.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success if hashes match, or an error if they don't.
    pub fn verify_root_hash(
        &self,
        context_id: &ContextId,
        claimed_hash: [u8; 32],
    ) -> eyre::Result<()> {
        let computed = self.compute_root_hash(context_id)?;

        if computed != claimed_hash {
            eyre::bail!(
                "Root hash verification failed for context {}: computed {} != claimed {}",
                context_id,
                hex::encode(computed),
                hex::encode(claimed_hash)
            );
        }

        tracing::debug!(
            %context_id,
            hash = ?Hash::from(computed),
            "Root hash verified successfully"
        );

        Ok(())
    }

    /// Reads ROOT's index entry + `Key::Entry(ROOT)` bytes in a single
    /// pass and returns both the self-dump (own_hash/full_hash/entry
    /// summary) and the children list — diagnostic for #2319.
    ///
    /// Used by the hash-heartbeat handler when it observes "Same DAG
    /// heads but different root hash". Logging this dump from both
    /// peers lets a flake be triaged by diff: matching children +
    /// mismatched `own_hash` → ROOT-entity write-path divergence;
    /// mismatched children → subtree divergence at the listed entity.
    ///
    /// One RocksDB read per key (Index + Entry), one borsh
    /// deserialize. Returns `Ok(None)` if ROOT has no index entry
    /// (empty state); `entry_bytes_hash` is `None` if the Index entry
    /// exists but Key::Entry(ROOT) is absent.
    pub fn dump_root(
        &self,
        context_id: &ContextId,
    ) -> eyre::Result<Option<(RootSelfDump, Vec<RootChildDump>)>> {
        let idx_state_key = Self::index_state_key(context_id);
        let handle = self.datastore.handle();
        let idx_db_key = key::ContextState::new(*context_id, idx_state_key);
        let Some(idx_data) = handle.get(&idx_db_key)? else {
            return Ok(None);
        };
        let idx_bytes: Vec<u8> = idx_data.as_ref().to_vec();
        drop(idx_data);

        let mut reader: &[u8] = &idx_bytes;
        let index = borsh_layout::EntityIndex::deserialize_reader(&mut reader)
            .map_err(|e| eyre::eyre!("dump_root: EntityIndex deserialize failed: {e}"))?;

        let children: Vec<RootChildDump> = index
            .children
            .unwrap_or_default()
            .into_iter()
            .map(|c| RootChildDump {
                id: c.id,
                merkle_hash: c.merkle_hash,
                created_at: c.metadata.created_at,
                updated_at: c.metadata.updated_at,
                crdt_type: c.metadata.crdt_type,
                field_name: c.metadata.field_name,
            })
            .collect();

        let entry_state_key = Self::entry_state_key(context_id);
        let entry_db_key = key::ContextState::new(*context_id, entry_state_key);
        let (entry_bytes_hash, entry_bytes_len) = match handle.get(&entry_db_key)? {
            Some(entry_data) => {
                let entry_bytes: Vec<u8> = entry_data.as_ref().to_vec();
                drop(entry_data);
                let h: [u8; 32] = Sha256::digest(&entry_bytes).into();
                (Some(h), entry_bytes.len())
            }
            None => (None, 0),
        };

        let self_dump = RootSelfDump {
            own_hash: index.own_hash,
            full_hash: index.full_hash,
            entry_bytes_hash,
            entry_bytes_len,
            children_count: children.len(),
        };

        Ok(Some((self_dump, children)))
    }

    /// Returns a stream of all context IDs stored locally.
    ///
    /// # Arguments
    ///
    /// * `start` - An optional `ContextId` from which to begin the stream. If `None`,
    ///    the stream starts from the beginning.
    ///
    /// # Returns
    ///
    /// An implementation of `Stream` that yields `Result<ContextId>`.
    pub fn get_context_ids(
        &self,
        start: Option<ContextId>,
    ) -> impl Stream<Item = eyre::Result<ContextId>> {
        let handle = self.datastore.handle();

        try_stream! {
            let mut iter = handle.iter::<key::ContextMeta>()?;

            let start = start.and_then(|s| iter.seek(key::ContextMeta::new(s)).transpose());

            for key in start.into_iter().chain(iter.keys()) {
                yield key?.context_id();
            }
        }
    }

    /// Checks if a given public key is a member of a context in the local datastore.
    ///
    /// # Arguments
    ///
    /// * `context_id` - The context to check within.
    /// * `public_key` - The public key of the potential member.
    ///
    /// # Returns
    ///
    /// A `Result` containing `true` if the identity is a known member, `false` otherwise.
    pub fn has_member(&self, context_id: &ContextId, public_key: &PublicKey) -> eyre::Result<bool> {
        let handle = self.datastore.handle();

        // Check ContextIdentity first (fast path, covers locally-written entries).
        let ci_key = key::ContextIdentity::new(*context_id, *public_key);
        if handle.has(&ci_key)? {
            return Ok(true);
        }

        // Fall back to group membership: if the identity is a member of the
        // group that owns this context, they are implicitly a context member.
        let ref_key = key::ContextGroupRef::new(*context_id);
        if let Some(group_id_bytes) = handle.get(&ref_key)? {
            let gm_key = key::GroupMember::new(group_id_bytes, *public_key);
            if handle.has(&gm_key)? {
                return Ok(true);
            }

            // The group admin/creator never publishes a MemberJoined governance
            // op for themselves, so joining nodes never store a GroupMember entry
            // for the creator. Fall back to GroupMeta.admin_identity so that the
            // creator's identity is recognised as a valid member on all nodes.
            let meta_key = key::GroupMeta::new(group_id_bytes);
            if let Some(meta) = handle.get(&meta_key)? {
                let meta: key::GroupMetaValue = meta;
                if meta.admin_identity == *public_key {
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }

    /// Returns the group/namespace ID for a context, if the context is owned by a group.
    ///
    /// The sync manager uses this as a fallback for peer discovery when the
    /// context-specific gossipsub mesh has not formed yet (e.g., just after a join).
    /// The namespace gossipsub topic establishes its mesh during join with a grace period,
    /// so namespace peers are reachable for direct-stream context sync even before the
    /// context topic mesh exists.
    pub fn get_context_group_id(&self, context_id: &ContextId) -> eyre::Result<Option<[u8; 32]>> {
        let handle = self.datastore.handle();
        let ref_key = key::ContextGroupRef::new(*context_id);
        Ok(handle.get(&ref_key)?)
    }

    /// Retrieves and returns a stream of all members of a given context.
    ///
    /// # Arguments
    ///
    /// * `context_id` - The context to query for members.
    /// * `owned` - If `Some(true)`, the stream returns only members for which this node holds
    ///    the private key. If `Some(false)` or `None`, it returns all members.
    ///
    /// # Returns
    ///
    /// A stream of tuples `(PublicKey, bool)`, where the boolean indicates if the identity is owned.
    pub fn get_context_members(
        &self,
        context_id: &ContextId,
        owned: Option<bool>,
    ) -> impl Stream<Item = eyre::Result<(PublicKey, bool)>> {
        let handle = self.datastore.handle();
        let context_id = *context_id;
        let only_owned = owned.unwrap_or(false);

        try_stream! {
            let mut iter = handle.iter::<key::ContextIdentity>()?;

            let first = iter
                .seek(key::ContextIdentity::new(context_id, [0; DIGEST_SIZE].into()))
                .transpose()
                .map(|k| (k, iter.read()));

            for (k, v) in first.into_iter().chain(iter.entries()) {
                let (k, v) = (k?, v?);

                if k.context_id() != context_id {
                    break;
                }

                let is_owned = v.private_key.is_some();
                if !only_owned || is_owned {
                    yield (k.public_key(), is_owned);
                }
            }
        }
    }
}

/// A client for interacting with the context management system.
///
/// This struct serves as the primary public API, providing methods to create,
/// join, query, and manage contexts and their members. It orchestrates
/// interactions between the datastore, background actors, and external networks.
#[derive(Clone, Debug)]
pub struct ContextClient {
    /// A registry providing synchronous read/write access to context metadata.
    registry: ContextRegistry,
    /// A client for communicating with the underlying Calimero node.
    node_client: NodeClient,
    /// A lazy-initialized sender handle to the `ContextManager` actor. This is used
    /// to send asynchronous messages for processing.
    context_manager: LazyRecipient<ContextMessage>,
    /// Routes incoming `SignedAck` messages from the gossipsub receiver
    /// to the in-flight `publish_and_await_ack` caller waiting on a
    /// specific `op_hash`. Shared with `ContextManager` (which wires
    /// publish-side subscriptions) — both hold a clone of the same Arc
    /// so acks routed here reach the awaiter without an actor mailbox
    /// hop. See `calimero_context::governance_broadcast`.
    ack_router: Arc<AckRouter>,
}

/// Generates a simple async send method on `ContextClient` that forwards a request
/// to the `ContextManager` actor via `ContextMessage` and awaits the response.
///
/// Usage: `forward_to_actor!(method_name, VariantName, RequestType, ReturnType);`
macro_rules! forward_to_actor {
    ($method:ident, $variant:ident, $request_ty:ty, $return_ty:ty) => {
        pub async fn $method(&self, request: $request_ty) -> $return_ty {
            let (sender, receiver) = oneshot::channel();
            self.context_manager
                .send(ContextMessage::$variant {
                    request,
                    outcome: sender,
                })
                .await
                .expect("Mailbox not to be dropped");
            receiver.await.expect("Mailbox not to be dropped")
        }
    };
}

impl ContextClient {
    #[must_use]
    pub fn new(
        datastore: Store,
        node_client: NodeClient,
        context_manager: LazyRecipient<ContextMessage>,
    ) -> Self {
        Self {
            registry: ContextRegistry::new(datastore),
            node_client,
            context_manager,
            ack_router: Arc::new(AckRouter::default()),
        }
    }

    /// Shared `AckRouter` for the three-phase governance contract. The
    /// gossipsub receiver in calimero-node calls `route()` on this when
    /// it sees a `NamespaceTopicMsg::Ack`; `publish_and_await_ack`
    /// inside calimero-context subscribes via the same instance.
    #[must_use]
    pub fn ack_router(&self) -> &Arc<AckRouter> {
        &self.ack_router
    }

    /// Returns a reference to the underlying `ContextRegistry`.
    pub const fn registry(&self) -> &ContextRegistry {
        &self.registry
    }

    /// Returns a handle to the datastore for direct access.
    /// Used by node components that need to read stored data.
    pub fn datastore_handle(&self) -> calimero_store::Handle<Store> {
        self.registry.datastore_handle()
    }

    /// Returns a reference to the underlying `Store`.
    /// Used by governance operations that need direct store access.
    pub fn datastore(&self) -> &Store {
        self.registry.datastore()
    }

    pub(crate) const fn node_client(&self) -> &NodeClient {
        &self.node_client
    }

    /// Sends a request to create a new context.
    ///
    /// This operation is asynchronous and is handled by the `ContextManager` actor.
    ///
    /// # Arguments
    ///
    /// * `protocol` - The name of the protocol that will be used for the new context.
    /// * `application_id` - The ID of the application that will run in the context.
    /// * `identity_secret` - An optional private key to use for the initial identity. If not
    ///   provided, a new identity will be generated.
    /// * `init_params` - Raw byte parameters for initializing the application state.
    /// * `seed` - An optional 32-byte seed for deterministic context ID and identity creation.
    ///
    /// # Returns
    ///
    /// A `Result` containing the `CreateContextResponse` from the actor upon completion.
    pub async fn create_context(
        &self,
        protocol: String,
        application_id: &ApplicationId,
        service_name: Option<String>,
        identity_secret: Option<PrivateKey>,
        init_params: Vec<u8>,
        seed: Option<[u8; DIGEST_SIZE]>,
        group_id: ContextGroupId,
        name: Option<String>,
    ) -> eyre::Result<CreateContextResponse> {
        let (sender, receiver) = oneshot::channel();

        self.context_manager
            .send(ContextMessage::CreateContext {
                request: CreateContextRequest {
                    protocol,
                    seed,
                    application_id: *application_id,
                    service_name,
                    identity_secret,
                    init_params,
                    group_id,
                    name,
                },
                outcome: sender,
            })
            .await
            .expect("Mailbox not to be dropped");

        receiver.await.expect("Mailbox not to be dropped")
    }

    /// Creates and signs a one-time, expiring open invitation for a new member.
    ///
    /// This method allows an existing member of a context (the inviter) to generate a
    /// shareable invitation. The method fetches the inviter's private key managed
    /// by the local node, signs the invitation details, and returns the resulting
    /// payload and signature.
    ///
    /// # Arguments
    /// * `context_id` - The context to invite the new member to.
    /// * `inviter_id` - The public key of the existing member creating the invitation.
    ///                  This node must have the corresponding private key for this identity.
    /// * `valid_for_seconds` - How long (in seconds) the invitation remains valid.
    /// * `_secret_salt` - Unused; a fresh random salt is generated internally.
    ///
    /// # Returns
    /// * A `Result` containing the `SignedOpenInvitation` if successful, or an error if
    /// the inviter's private key is not found or signing fails.
    /// * Returns `Ok(None)` if the context configuration cannot be found locally.
    pub async fn invite_member(
        &self,
        context_id: &ContextId,
        inviter_id: &PublicKey,
        valid_for_seconds: u64,
        _secret_salt: [u8; DIGEST_SIZE],
    ) -> eyre::Result<Option<SignedOpenInvitation>> {
        let secret_salt = {
            let mut rng = rand::thread_rng();
            rng.gen::<[u8; DIGEST_SIZE]>()
        };

        let ctx_exists = self.has_context(context_id)?;
        tracing::info!(
            %context_id,
            %inviter_id,
            ctx_exists,
            "invite_member: starting"
        );
        if !ctx_exists {
            return Ok(None);
        };

        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock before epoch")
            .as_secs();
        let expiration_timestamp = now_secs + valid_for_seconds;

        let inviter_identity = self
            .get_identity(context_id, inviter_id)?
            .with_context(|| {
                format!("Inviter identity {inviter_id} not found for context {context_id}")
            })?;
        let inviter_private_key = inviter_identity.private_key().map_err(|e| {
            eyre::eyre!("inviter {inviter_id} has no private key for context {context_id}: {e}")
        })?;

        let inviter_identity: [u8; DIGEST_SIZE] = **inviter_id;
        let inviter_identity_context_type = inviter_identity.into();
        let context_id = **context_id;

        let invitation = InvitationFromMember {
            inviter_identity: inviter_identity_context_type,
            context_id: context_id.into(),
            expiration_timestamp,
            secret_salt,
        };

        let invitation_bytes =
            borsh::to_vec(&invitation).context("Failed to serialize invitation")?;
        let hash = Sha256::digest(&invitation_bytes);
        let signature = inviter_private_key.sign(&hash).context("Signing failed")?;

        let (application_id, blob_id, source) = {
            let ctx_id = calimero_primitives::context::ContextId::from(context_id);
            match self.get_context_application(&ctx_id).await {
                Ok(app) => (
                    Some(*app.id),
                    Some(*app.blob.bytecode),
                    Some(app.source.to_string()),
                ),
                Err(_) => (None, None, None),
            }
        };

        let group_id = {
            let ctx_id = calimero_primitives::context::ContextId::from(context_id);
            let handle = self.registry.datastore.handle();
            handle.get(&key::ContextGroupRef::new(ctx_id))?
        };

        tracing::info!(
            ?application_id,
            ?blob_id,
            ?source,
            ?group_id,
            "invite_member: populated invitation metadata"
        );

        Ok(Some(SignedOpenInvitation {
            invitation,
            inviter_signature: hex::encode(signature.to_bytes()),
            application_id,
            blob_id,
            source,
            group_id,
        }))
    }

    // --- Delegation methods for ContextRegistry ---

    /// Checks if a context's metadata exists in the local datastore.
    pub fn has_context(&self, context_id: &ContextId) -> eyre::Result<bool> {
        self.registry.has_context(context_id)
    }

    /// The version of the blob this context actually EXECUTES, when it
    /// differs from the application row's. The activation marker is the
    /// per-context truth; the row is a download cache holding the latest
    /// install, so under multi-version coexistence the two diverge.
    /// `None` ⇒ the row's version already tells the truth.
    pub async fn executing_application_version(&self, context_id: &ContextId) -> Option<String> {
        let marker = {
            let handle = self.registry.datastore.handle();
            let marker = handle
                .get(&key::ContextActivatedBlob::new(*context_id))
                .ok()
                .flatten()?
                .blob;
            let row_blob = handle
                .get(&key::ContextMeta::new(*context_id))
                .ok()
                .flatten()
                .and_then(|m| handle.get(&m.application).ok().flatten())
                .map(|app| *app.bytecode.blob_id().as_ref());
            if row_blob == Some(marker) {
                return None;
            }
            marker
        };
        self.node_client
            .blob_app_version(&calimero_primitives::blobs::BlobId::from(marker))
            .await
    }

    /// Retrieves a context metadata from the local datastore.
    pub fn get_context(&self, context_id: &ContextId) -> eyre::Result<Option<Context>> {
        self.registry.get_context(context_id)
    }

    /// Updates the DAG heads for a context after applying a delta.
    pub fn update_dag_heads(
        &self,
        context_id: &ContextId,
        dag_heads: Vec<[u8; 32]>,
    ) -> eyre::Result<()> {
        self.registry.update_dag_heads(context_id, dag_heads)
    }

    /// See [`ContextRegistry::persist_deltas_and_dag_heads`].
    pub fn persist_deltas_and_dag_heads(
        &self,
        context_id: &ContextId,
        deltas: &[(key::ContextDagDelta, types::ContextDagDelta)],
        dag_heads: Vec<[u8; 32]>,
    ) -> eyre::Result<()> {
        self.registry
            .persist_deltas_and_dag_heads(context_id, deltas, dag_heads)
    }

    /// See [`ContextRegistry::persist_delta_records`].
    pub fn persist_delta_records(
        &self,
        deltas: &[(key::ContextDagDelta, types::ContextDagDelta)],
    ) -> eyre::Result<()> {
        self.registry.persist_delta_records(deltas)
    }

    /// Atomically deletes a batch of DAG delta rows (issue #2026 compaction).
    /// Delegates to [`ContextRegistry::prune_delta_records`].
    pub fn prune_delta_records(&self, delta_keys: &[key::ContextDagDelta]) -> eyre::Result<()> {
        self.registry.prune_delta_records(delta_keys)
    }

    /// Updates the ApplicationId for a context.
    pub fn update_context_application_id(
        &self,
        context_id: &ContextId,
        application_id: ApplicationId,
    ) -> eyre::Result<()> {
        self.registry
            .update_context_application_id(context_id, application_id)
    }

    /// Computes the actual root hash from storage by reading the root Index entry.
    pub fn compute_root_hash(&self, context_id: &ContextId) -> eyre::Result<[u8; 32]> {
        self.registry.compute_root_hash(context_id)
    }

    /// Forces the root hash for a context to a specific value.
    pub fn force_root_hash(&self, context_id: &ContextId, root_hash: Hash) -> eyre::Result<()> {
        self.registry.force_root_hash(context_id, root_hash)
    }

    /// Verifies that the stored root hash matches the actual state.
    pub fn verify_root_hash(
        &self,
        context_id: &ContextId,
        claimed_hash: [u8; 32],
    ) -> eyre::Result<()> {
        self.registry.verify_root_hash(context_id, claimed_hash)
    }

    /// Diagnostic — dump ROOT's self summary + children list in one read.
    /// See [`ContextRegistry::dump_root`].
    pub fn dump_root(
        &self,
        context_id: &ContextId,
    ) -> eyre::Result<Option<(RootSelfDump, Vec<RootChildDump>)>> {
        self.registry.dump_root(context_id)
    }

    /// Returns a stream of all context IDs stored locally.
    pub fn get_context_ids(
        &self,
        start: Option<ContextId>,
    ) -> impl Stream<Item = eyre::Result<ContextId>> {
        self.registry.get_context_ids(start)
    }

    /// Checks if a given public key is a member of a context in the local datastore.
    pub fn has_member(&self, context_id: &ContextId, public_key: &PublicKey) -> eyre::Result<bool> {
        self.registry.has_member(context_id, public_key)
    }

    /// Returns the group/namespace ID for a context, if the context belongs to a group.
    pub fn get_context_group_id(&self, context_id: &ContextId) -> eyre::Result<Option<[u8; 32]>> {
        self.registry.get_context_group_id(context_id)
    }

    /// Retrieves and returns a stream of all members of a given context.
    pub fn get_context_members(
        &self,
        context_id: &ContextId,
        owned: Option<bool>,
    ) -> impl Stream<Item = eyre::Result<(PublicKey, bool)>> {
        self.registry.get_context_members(context_id, owned)
    }

    /// Sends a request to execute a method within a context.
    ///
    /// This is the primary way to interact with the application running inside a context.
    /// The request is handled asynchronously by the `ContextManager` actor.
    ///
    /// # Arguments
    ///
    /// * `context_id` - The ID of the context where the execution should occur.
    /// * `executor` - The public key of the identity performing the execution. The executor
    ///   must be a member of the context.
    /// * `method` - The string name of the application method to call.
    /// * `payload` - The input data (e.g., serialized JSON) for the method.
    /// * `aliases` - A list of public key aliases to use for this specific execution.
    /// * `atomic` - An optional handle for batching multiple executions into an atomic transaction.
    ///
    /// # Returns
    ///
    /// A `Result` containing the `ExecuteResponse` on success, or an `ExecuteError` on failure.
    pub async fn execute(
        &self,
        context_id: &ContextId,
        executor: &PublicKey,
        method: String,
        payload: Vec<u8>,
        aliases: Vec<Alias<PublicKey>>,
        atomic: Option<ContextAtomic>,
    ) -> Result<ExecuteResponse, ExecuteError> {
        let (sender, receiver) = oneshot::channel();

        self.context_manager
            .send(ContextMessage::Execute {
                request: ExecuteRequest {
                    context: *context_id,
                    executor: *executor,
                    method,
                    payload,
                    aliases,
                    atomic,
                },
                outcome: sender,
            })
            .await
            .expect("Mailbox not to be dropped");

        receiver.await.expect("Mailbox not to be dropped")
    }

    /// Acquire the per-context execution lock and return its owned guard.
    ///
    /// This is the same `Arc<Mutex<ContextId>>` the executor holds for a WASM
    /// run. Host-side storage mutations that bypass the executor — the sync
    /// session's `EntityPush` / `EntityDeletePush` apply paths — must hold this
    /// guard for the duration of their apply so they cannot interleave with a
    /// concurrent `__calimero_sync_next` delta merge (which would record a torn
    /// root hash that delta-sync can't repair). Drop the guard before invoking
    /// anything that re-enters the executor (e.g. `merge_root_state`), or it
    /// will deadlock against itself.
    ///
    /// Returns `None` for an unknown context; the caller then applies
    /// best-effort without serialization (the apply no-ops on a missing
    /// context anyway).
    pub async fn acquire_lock(&self, context_id: &ContextId) -> Option<ContextAtomicKey> {
        let (sender, receiver) = oneshot::channel();

        self.context_manager
            .send(ContextMessage::AcquireContextLock {
                request: AcquireContextLockRequest {
                    context: *context_id,
                },
                outcome: sender,
            })
            .await
            .expect("Mailbox not to be dropped");

        receiver.await.expect("Mailbox not to be dropped")
    }

    /// Invoke the app's typed root-state CRDT merge inside WASM and return
    /// the merged bytes.
    ///
    /// The host can't deserialize the app's root state (it doesn't have
    /// the type at compile time), so any sync path that needs to merge
    /// two root-state byte blobs sends them into the WASM module via the
    /// macro-generated `__calimero_merge_root_state` export, which knows
    /// the type and dispatches `Mergeable::merge`. This is the
    /// receive-side counterpart to per-action signature verification:
    /// where signatures verify "did this writer authorize this byte
    /// blob," `merge_root_state` answers "what does the app's CRDT say
    /// these two byte blobs combine to."
    ///
    /// Returns the merged bytes on success. Returns
    /// `ExecuteError::InternalError` if the WASM merge function returned
    /// an error variant, the payload didn't round-trip through the wire
    /// format, or the WASM module doesn't export the entry point (which
    /// means the app didn't use `#[app::state]` — an upgrade gate
    /// concern, not a runtime sync concern).
    pub async fn merge_root_state(
        &self,
        context_id: &ContextId,
        executor: &PublicKey,
        request: calimero_storage::merge::MergeRootStateRequest,
    ) -> Result<Vec<u8>, ExecuteError> {
        let payload = borsh::to_vec(&request).map_err(|err| {
            tracing::error!(
                %context_id,
                %err,
                "merge_root_state: failed to serialize MergeRootStateRequest"
            );
            ExecuteError::InternalError
        })?;

        let response = self
            .execute(
                context_id,
                executor,
                "__calimero_merge_root_state".to_owned(),
                payload,
                vec![],
                None,
            )
            .await?;

        let return_bytes = match response.returns {
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                tracing::error!(
                    %context_id,
                    "merge_root_state: WASM export returned no bytes"
                );
                return Err(ExecuteError::InternalError);
            }
            Err(err) => {
                tracing::error!(
                    %context_id,
                    ?err,
                    "merge_root_state: WASM export reported a function-call error"
                );
                return Err(ExecuteError::InternalError);
            }
        };

        let response: calimero_storage::merge::MergeRootStateResponse =
            borsh::from_slice(&return_bytes).map_err(|err| {
                tracing::error!(
                    %context_id,
                    %err,
                    "merge_root_state: failed to deserialize MergeRootStateResponse"
                );
                ExecuteError::InternalError
            })?;

        match response {
            calimero_storage::merge::MergeRootStateResponse::Ok(bytes) => Ok(bytes),
            calimero_storage::merge::MergeRootStateResponse::Err(msg) => {
                tracing::error!(
                    %context_id,
                    error = %msg,
                    "merge_root_state: WASM Mergeable::merge returned an error"
                );
                Err(ExecuteError::InternalError)
            }
        }
    }

    /// Sends a request to update the application for a given context.
    /// This is an asynchronous operation handled by the `ContextManager` actor.
    ///
    /// # Arguments
    /// * `context_id` - The ID of the context where to update the application.
    /// * `application_id` - The ID of the new application to switch to.
    /// * `identity` - The public key of the member authorizing the update.
    /// * `migrate_method` - Optional name of the migration function to execute.
    ///
    /// # Returns
    ///
    /// An empty `Result` indicating the outcome of the application update request.
    pub async fn update_application(
        &self,
        context_id: &ContextId,
        application_id: &ApplicationId,
        identity: &PublicKey,
        migrate_method: Option<String>,
    ) -> eyre::Result<()> {
        let (sender, receiver) = oneshot::channel();

        let migration = migrate_method.map(|method| MigrationParams { method });

        self.context_manager
            .send(ContextMessage::UpdateApplication {
                request: UpdateApplicationRequest {
                    context_id: *context_id,
                    application_id: *application_id,
                    public_key: *identity,
                    migration,
                },
                outcome: sender,
            })
            .await
            .expect("Mailbox not to be dropped");

        receiver.await.expect("Mailbox not to be dropped")
    }

    /// Sends a request to delete a context from the local node.
    /// This is an asynchronous operation handled by the `ContextManager` actor. It will remove
    /// all associated data for the context from the local datastore.
    ///
    /// # Arguments
    /// * `context_id` - The ID of the context to delete.
    ///
    /// # Returns
    ///
    ///A `Result` containing the `DeleteContextResponse` from the actor.
    pub async fn delete_context(
        &self,
        context_id: &ContextId,
        requester: Option<PublicKey>,
    ) -> eyre::Result<DeleteContextResponse> {
        let (sender, receiver) = oneshot::channel();

        self.context_manager
            .send(ContextMessage::DeleteContext {
                request: DeleteContextRequest {
                    context_id: *context_id,
                    requester,
                },
                outcome: sender,
            })
            .await
            .expect("Mailbox not to be dropped");

        receiver.await.expect("Mailbox not to be dropped")
    }

    // --- Simple forwarding methods (generated by forward_to_actor! macro) ---
    forward_to_actor!(
        create_group,
        CreateGroup,
        CreateGroupRequest,
        eyre::Result<CreateGroupResponse>
    );
    forward_to_actor!(
        delete_group,
        DeleteGroup,
        DeleteGroupRequest,
        eyre::Result<DeleteGroupResponse>
    );
    forward_to_actor!(
        delete_namespace,
        DeleteNamespace,
        DeleteNamespaceRequest,
        eyre::Result<DeleteNamespaceResponse>
    );
    forward_to_actor!(
        add_group_members,
        AddGroupMembers,
        AddGroupMembersRequest,
        eyre::Result<()>
    );
    forward_to_actor!(
        remove_group_members,
        RemoveGroupMembers,
        RemoveGroupMembersRequest,
        eyre::Result<()>
    );
    forward_to_actor!(
        get_group_info,
        GetGroupInfo,
        GetGroupInfoRequest,
        eyre::Result<GroupInfoResponse>
    );
    forward_to_actor!(
        list_group_members,
        ListGroupMembers,
        ListGroupMembersRequest,
        eyre::Result<ListGroupMembersResponse>
    );
    forward_to_actor!(
        list_group_contexts,
        ListGroupContexts,
        ListGroupContextsRequest,
        eyre::Result<Vec<GroupContextEntry>>
    );
    forward_to_actor!(
        store_context_metadata,
        StoreContextMetadata,
        StoreContextMetadataRequest,
        eyre::Result<()>
    );
    forward_to_actor!(
        broadcast_group_local_state,
        BroadcastGroupLocalState,
        BroadcastGroupLocalStateRequest,
        eyre::Result<()>
    );
    forward_to_actor!(
        store_member_capability,
        StoreMemberCapability,
        StoreMemberCapabilityRequest,
        eyre::Result<()>
    );
    forward_to_actor!(
        store_default_capabilities,
        StoreDefaultCapabilities,
        StoreDefaultCapabilitiesRequest,
        eyre::Result<()>
    );
    forward_to_actor!(
        store_subgroup_visibility,
        StoreSubgroupVisibility,
        StoreSubgroupVisibilityRequest,
        eyre::Result<()>
    );
    forward_to_actor!(
        set_member_metadata,
        SetMemberMetadata,
        SetMemberMetadataRequest,
        eyre::Result<()>
    );
    forward_to_actor!(
        store_member_metadata,
        StoreMemberMetadata,
        StoreMemberMetadataRequest,
        eyre::Result<()>
    );
    forward_to_actor!(
        set_group_metadata,
        SetGroupMetadata,
        SetGroupMetadataRequest,
        eyre::Result<()>
    );
    forward_to_actor!(
        store_group_metadata,
        StoreGroupMetadata,
        StoreGroupMetadataRequest,
        eyre::Result<()>
    );
    forward_to_actor!(
        set_context_metadata,
        SetContextMetadata,
        SetContextMetadataRequest,
        eyre::Result<()>
    );
    forward_to_actor!(
        get_group_metadata,
        GetGroupMetadata,
        GetGroupMetadataRequest,
        eyre::Result<Option<MetadataRecord>>
    );
    forward_to_actor!(
        get_member_metadata,
        GetMemberMetadata,
        GetMemberMetadataRequest,
        eyre::Result<Option<MetadataRecord>>
    );
    forward_to_actor!(
        get_context_metadata,
        GetContextMetadata,
        GetContextMetadataRequest,
        eyre::Result<Option<MetadataRecord>>
    );
    forward_to_actor!(
        store_group_context,
        StoreGroupContext,
        StoreGroupContextRequest,
        eyre::Result<()>
    );
    forward_to_actor!(
        store_group_meta,
        StoreGroupMeta,
        StoreGroupMetaRequest,
        eyre::Result<()>
    );
    forward_to_actor!(
        upgrade_group,
        UpgradeGroup,
        UpgradeGroupRequest,
        eyre::Result<UpgradeGroupResponse>
    );
    forward_to_actor!(
        get_group_upgrade_status,
        GetGroupUpgradeStatus,
        GetGroupUpgradeStatusRequest,
        eyre::Result<Option<GroupUpgradeInfo>>
    );
    forward_to_actor!(
        retry_group_upgrade,
        RetryGroupUpgrade,
        RetryGroupUpgradeRequest,
        eyre::Result<UpgradeGroupResponse>
    );
    forward_to_actor!(
        create_group_invitation,
        CreateGroupInvitation,
        CreateGroupInvitationRequest,
        eyre::Result<CreateGroupInvitationResponse>
    );
    forward_to_actor!(
        join_group,
        JoinGroup,
        JoinGroupRequest,
        eyre::Result<JoinGroupResponse>
    );
    forward_to_actor!(
        list_all_groups,
        ListAllGroups,
        ListAllGroupsRequest,
        eyre::Result<Vec<GroupSummary>>
    );
    forward_to_actor!(
        update_group_settings,
        UpdateGroupSettings,
        UpdateGroupSettingsRequest,
        eyre::Result<()>
    );
    forward_to_actor!(
        update_member_role,
        UpdateMemberRole,
        UpdateMemberRoleRequest,
        eyre::Result<()>
    );
    forward_to_actor!(
        detach_context_from_group,
        DetachContextFromGroup,
        DetachContextFromGroupRequest,
        eyre::Result<()>
    );
    forward_to_actor!(
        get_group_for_context,
        GetGroupForContext,
        GetGroupForContextRequest,
        eyre::Result<Option<ContextGroupId>>
    );
    forward_to_actor!(
        sync_group,
        SyncGroup,
        SyncGroupRequest,
        eyre::Result<SyncGroupResponse>
    );
    forward_to_actor!(
        join_context,
        JoinContext,
        JoinContextRequest,
        eyre::Result<JoinContextResponse>
    );
    forward_to_actor!(
        join_subgroup_inheritance,
        JoinSubgroupInheritance,
        JoinSubgroupInheritanceRequest,
        eyre::Result<JoinSubgroupInheritanceResponse>
    );
    forward_to_actor!(
        leave_context,
        LeaveContext,
        LeaveContextRequest,
        eyre::Result<LeaveContextResponse>
    );
    forward_to_actor!(
        leave_group,
        LeaveGroup,
        LeaveGroupRequest,
        eyre::Result<LeaveGroupResponse>
    );
    forward_to_actor!(
        issue_ownership_proof,
        IssueOwnershipProof,
        IssueOwnershipProofRequest,
        eyre::Result<IssueOwnershipProofResponse>
    );
    forward_to_actor!(
        issue_namespace_ownership_proof,
        IssueNamespaceOwnershipProof,
        IssueNamespaceOwnershipProofRequest,
        eyre::Result<IssueOwnershipProofResponse>
    );
    forward_to_actor!(
        leave_namespace,
        LeaveNamespace,
        LeaveNamespaceRequest,
        eyre::Result<LeaveNamespaceResponse>
    );
    forward_to_actor!(
        set_member_capabilities,
        SetMemberCapabilities,
        SetMemberCapabilitiesRequest,
        eyre::Result<()>
    );
    forward_to_actor!(
        set_member_auto_follow,
        SetMemberAutoFollow,
        SetMemberAutoFollowRequest,
        eyre::Result<()>
    );
    forward_to_actor!(
        get_member_capabilities,
        GetMemberCapabilities,
        GetMemberCapabilitiesRequest,
        eyre::Result<GetMemberCapabilitiesResponse>
    );
    forward_to_actor!(
        set_default_capabilities,
        SetDefaultCapabilities,
        SetDefaultCapabilitiesRequest,
        eyre::Result<()>
    );
    forward_to_actor!(
        set_tee_admission_policy,
        SetTeeAdmissionPolicy,
        SetTeeAdmissionPolicyRequest,
        eyre::Result<()>
    );
    forward_to_actor!(
        admit_tee_node,
        AdmitTeeNode,
        AdmitTeeNodeRequest,
        eyre::Result<()>
    );
    forward_to_actor!(
        set_subgroup_visibility,
        SetSubgroupVisibility,
        SetSubgroupVisibilityRequest,
        eyre::Result<()>
    );
    forward_to_actor!(
        list_namespaces,
        ListNamespaces,
        ListNamespacesRequest,
        eyre::Result<Vec<NamespaceSummary>>
    );
    forward_to_actor!(
        get_namespace_identity,
        GetNamespaceIdentity,
        GetNamespaceIdentityRequest,
        eyre::Result<Option<(ContextGroupId, PublicKey)>>
    );
    forward_to_actor!(
        list_namespaces_for_application,
        ListNamespacesForApplication,
        ListNamespacesForApplicationRequest,
        eyre::Result<Vec<NamespaceSummary>>
    );
    forward_to_actor!(
        get_cascade_status,
        GetCascadeStatus,
        GetCascadeStatusRequest,
        eyre::Result<Vec<CascadeStatusEntry>>
    );
    forward_to_actor!(
        get_migration_status,
        GetMigrationStatus,
        GetMigrationStatusRequest,
        eyre::Result<MigrationStatus>
    );
    forward_to_actor!(
        abort_migration,
        AbortMigration,
        AbortMigrationRequest,
        eyre::Result<AbortMigrationResponse>
    );

    // --- Methods with custom parameter handling (not suitable for forward_to_actor!) ---

    pub async fn apply_signed_group_op(
        &self,
        op: crate::local_governance::SignedGroupOp,
    ) -> eyre::Result<bool> {
        let (sender, receiver) = oneshot::channel();

        self.context_manager
            .send(ContextMessage::ApplySignedGroupOp {
                request: ApplySignedGroupOpRequest { op },
                outcome: sender,
            })
            .await
            .expect("Mailbox not to be dropped");

        receiver.await.expect("Mailbox not to be dropped")
    }

    /// Apply a signed namespace governance op to this node's local state.
    ///
    /// Returns a [`NamespaceApplyOutcome`] distinguishing the three success
    /// states: `Applied`, `Pending` (parents missing — caller should trigger
    /// backfill), and `Duplicate` (already present — no action required).
    pub async fn apply_signed_namespace_op(
        &self,
        op: crate::local_governance::SignedNamespaceOp,
    ) -> eyre::Result<NamespaceApplyOutcome> {
        let (sender, receiver) = oneshot::channel();

        self.context_manager
            .send(ContextMessage::ApplySignedNamespaceOp {
                request: ApplySignedNamespaceOpRequest { op },
                outcome: sender,
            })
            .await
            .expect("Mailbox not to be dropped");

        receiver.await.expect("Mailbox not to be dropped")
    }

    /// Returns the number of ops in this namespace's governance DAG whose
    /// parents have not yet been applied locally (the "pending" queue size).
    ///
    /// Used by the cross-peer parent-pull loop (#2198) to decide whether
    /// another backfill round against another mesh peer is needed.
    pub async fn namespace_pending_op_count(&self, namespace_id: [u8; 32]) -> eyre::Result<usize> {
        let (sender, receiver) = oneshot::channel();

        self.context_manager
            .send(ContextMessage::NamespacePendingOpCount {
                request: crate::messages::NamespacePendingOpCountRequest { namespace_id },
                outcome: sender,
            })
            .await
            .expect("Mailbox not to be dropped");

        receiver.await.expect("Mailbox not to be dropped")
    }
}

#[cfg(test)]
mod atomic_persist_tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use calimero_primitives::application::ApplicationId;
    use calimero_primitives::context::ContextId;
    use calimero_storage::logical_clock::HybridTimestamp;
    use calimero_store::config::StoreConfig;
    use calimero_store::db::{Column, Database, InMemoryDB};
    use calimero_store::iter::Iter;
    use calimero_store::slice::Slice;
    use calimero_store::tx::Transaction;
    use calimero_store::{key, types, Store};
    use eyre::Result as EyreResult;

    use super::ContextRegistry;

    const INITIAL_HEAD: [u8; 32] = [0x00; 32];
    const DELTA_A: [u8; 32] = [0x11; 32];
    const DELTA_B: [u8; 32] = [0x22; 32];

    fn ctx() -> ContextId {
        ContextId::from([0x07; 32])
    }

    fn seed_meta(store: &Store, context_id: &ContextId, heads: Vec<[u8; 32]>) {
        let key = key::ContextMeta::new(*context_id);
        let meta = types::ContextMeta::new(
            key::ApplicationMeta::new(ApplicationId::from([0xAA; 32])),
            [0u8; 32],
            heads,
            None,
        );
        let mut handle = store.handle();
        handle.put(&key, &meta).expect("seed context meta");
    }

    fn delta_record(
        context_id: &ContextId,
        id: [u8; 32],
    ) -> (key::ContextDagDelta, types::ContextDagDelta) {
        let key = key::ContextDagDelta::new(*context_id, id);
        let record = types::ContextDagDelta {
            delta_id: id,
            parents: Vec::new(),
            actions: vec![1, 2, 3],
            hlc: HybridTimestamp::zero(),
            applied: true,
            expected_root_hash: [0u8; 32],
            events: None,
            author_id: None,
            governance_position_blob: None,
            delta_signature: None,
        };
        (key, record)
    }

    fn read_heads(store: &Store, context_id: &ContextId) -> Vec<[u8; 32]> {
        store
            .handle()
            .get(&key::ContextMeta::new(*context_id))
            .expect("read meta")
            .expect("meta present")
            .dag_heads
    }

    fn has_delta(store: &Store, context_id: &ContextId, id: [u8; 32]) -> bool {
        store
            .handle()
            .get(&key::ContextDagDelta::new(*context_id, id))
            .expect("read delta")
            .is_some()
    }

    #[test]
    fn persists_all_deltas_and_heads_together() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let cid = ctx();
        seed_meta(&store, &cid, vec![INITIAL_HEAD]);

        let registry = ContextRegistry::new(store.clone());
        let deltas = [delta_record(&cid, DELTA_A), delta_record(&cid, DELTA_B)];

        registry
            .persist_deltas_and_dag_heads(&cid, &deltas, vec![DELTA_B])
            .expect("atomic persist succeeds");

        assert!(has_delta(&store, &cid, DELTA_A), "delta A persisted");
        assert!(has_delta(&store, &cid, DELTA_B), "delta B persisted");
        assert_eq!(read_heads(&store, &cid), vec![DELTA_B], "heads advanced");
    }

    /// A [`Database`] whose `apply` always fails, simulating a backend write
    /// error (disk full, RocksDB I/O fault) at commit time. Every other
    /// operation delegates to a real in-memory backend, so seeding and
    /// assertions observe genuine state.
    ///
    /// Once `armed` is set, `put` also fails. The atomic persist path must
    /// route all writes through `apply` (one transaction) — never a direct
    /// `put` — so arming the backend before the call under test turns any
    /// future regression that sneaks a stray `put` in ahead of the commit
    /// into a hard test failure rather than a silently-passing one. Seeding
    /// runs while disarmed.
    #[derive(Debug)]
    struct FailOnApply<D> {
        inner: D,
        armed: Arc<AtomicBool>,
    }

    impl<'a, D: Database<'a>> Database<'a> for FailOnApply<D> {
        fn open(_config: &StoreConfig) -> EyreResult<Self>
        where
            Self: Sized,
        {
            unimplemented!("test-only backend is constructed directly")
        }

        fn has(&self, col: Column, key: Slice<'_>) -> EyreResult<bool> {
            self.inner.has(col, key)
        }

        fn get(&self, col: Column, key: Slice<'_>) -> EyreResult<Option<Slice<'_>>> {
            self.inner.get(col, key)
        }

        fn put(&self, col: Column, key: Slice<'a>, value: Slice<'a>) -> EyreResult<()> {
            assert!(
                !self.armed.load(Ordering::SeqCst),
                "atomic persist must not write via direct `put`; all writes go through `apply`"
            );
            self.inner.put(col, key, value)
        }

        fn delete(&self, col: Column, key: Slice<'_>) -> EyreResult<()> {
            self.inner.delete(col, key)
        }

        fn iter(&self, col: Column) -> EyreResult<Iter<'_>> {
            self.inner.iter(col)
        }

        fn apply(&self, _tx: &Transaction<'a>) -> EyreResult<()> {
            eyre::bail!("injected apply failure")
        }
    }

    #[test]
    fn failed_commit_leaves_pre_cascade_state() {
        let armed = Arc::new(AtomicBool::new(false));
        let store = Store::new(Arc::new(FailOnApply {
            inner: InMemoryDB::owned(),
            armed: Arc::clone(&armed),
        }));
        let cid = ctx();
        seed_meta(&store, &cid, vec![INITIAL_HEAD]);

        // From here, any direct `put` is a bug — only `apply` may write.
        armed.store(true, Ordering::SeqCst);

        let registry = ContextRegistry::new(store.clone());
        let deltas = [delta_record(&cid, DELTA_A), delta_record(&cid, DELTA_B)];

        let err = registry
            .persist_deltas_and_dag_heads(&cid, &deltas, vec![DELTA_B])
            .expect_err("commit must surface the backend failure");
        assert!(
            err.to_string().contains("injected apply failure"),
            "unexpected error: {err}"
        );

        // Reads below are fine while armed (only `put` is gated).

        // All-or-nothing: neither delta nor the advanced heads landed.
        assert!(
            !has_delta(&store, &cid, DELTA_A),
            "delta A must not persist"
        );
        assert!(
            !has_delta(&store, &cid, DELTA_B),
            "delta B must not persist"
        );
        assert_eq!(
            read_heads(&store, &cid),
            vec![INITIAL_HEAD],
            "heads must remain at pre-cascade value"
        );
    }

    #[test]
    fn persist_delta_records_is_all_or_nothing() {
        let cid = ctx();

        // Happy path: every record lands; no meta read/heads write involved.
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let registry = ContextRegistry::new(store.clone());
        registry
            .persist_delta_records(&[delta_record(&cid, DELTA_A), delta_record(&cid, DELTA_B)])
            .expect("records persist");
        assert!(has_delta(&store, &cid, DELTA_A), "delta A persisted");
        assert!(has_delta(&store, &cid, DELTA_B), "delta B persisted");

        // Failure path: a backend whose `apply` fails leaves nothing behind.
        // Armed from the start — this method never reads or seeds, so no
        // `put` should occur (all writes go through `apply`).
        let armed = Arc::new(AtomicBool::new(true));
        let failing = Store::new(Arc::new(FailOnApply {
            inner: InMemoryDB::owned(),
            armed: Arc::clone(&armed),
        }));
        let failing_registry = ContextRegistry::new(failing.clone());
        let err = failing_registry
            .persist_delta_records(&[delta_record(&cid, DELTA_A)])
            .expect_err("commit must surface the backend failure");
        assert!(
            err.to_string().contains("injected apply failure"),
            "unexpected error: {err}"
        );
        assert!(
            !has_delta(&failing, &cid, DELTA_A),
            "delta A must not persist"
        );
    }

    #[test]
    fn store_batch_stages_until_commit_and_discards_on_drop() {
        use calimero_store::StoreBatch;

        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let cid = ctx();

        // Staged puts are invisible until commit.
        let (key_a, rec_a) = delta_record(&cid, DELTA_A);
        let mut batch = StoreBatch::new(&store);
        batch.put(&key_a, &rec_a).expect("stage put");
        assert!(
            !has_delta(&store, &cid, DELTA_A),
            "staged put must not be visible before commit"
        );
        batch.commit().expect("commit batch");

        // Round-trip fidelity: a record staged via `StoreBatch::put` (which
        // writes through `Transaction::raw_put` with `K::column()` +
        // `key.as_key().as_bytes()`) must read back byte-for-byte via the
        // typed `Handle::get` path. This pins down that the raw key/column
        // encoding matches what `Handle::put` would have written.
        let read_back = store
            .handle()
            .get(&key_a)
            .expect("read staged record")
            .expect("record present after commit");
        assert_eq!(read_back.delta_id, rec_a.delta_id, "delta_id round-trips");
        assert_eq!(read_back.actions, rec_a.actions, "actions round-trip");

        // A batch dropped without commit writes nothing.
        let (key_b, rec_b) = delta_record(&cid, DELTA_B);
        let mut dropped = StoreBatch::new(&store);
        dropped.put(&key_b, &rec_b).expect("stage put");
        drop(dropped);
        assert!(
            !has_delta(&store, &cid, DELTA_B),
            "dropped (uncommitted) batch must not persist"
        );
    }
}

#[cfg(test)]
mod get_context_version_tests {
    use std::sync::Arc;

    use calimero_primitives::application::ApplicationId;
    use calimero_primitives::context::ContextId;
    use calimero_store::db::InMemoryDB;
    use calimero_store::{key, types, Store};

    use super::ContextRegistry;

    // get_context resolves ApplicationMeta.version (semver) onto Context.
    #[test]
    fn get_context_carries_application_version() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let app_id = ApplicationId::from([0xAA; 32]);
        let app_key = key::ApplicationMeta::new(app_id);

        let app_meta = types::ApplicationMeta::new(
            key::BlobMeta::new([1u8; 32].into()),
            1024,
            "file://test.wasm".into(),
            vec![].into(),
            key::BlobMeta::new([2u8; 32].into()),
            "com.test.app".into(),
            "2.1.0".into(),
            "signer".into(),
        );
        let cid = ContextId::from([0x07; 32]);
        let ctx_meta = types::ContextMeta::new(app_key, [0u8; 32], vec![], None);
        {
            let mut handle = store.handle();
            handle.put(&app_key, &app_meta).expect("seed app meta");
            handle
                .put(&key::ContextMeta::new(cid), &ctx_meta)
                .expect("seed ctx meta");
        }

        let registry = ContextRegistry::new(store.clone());
        let ctx = registry
            .get_context(&cid)
            .expect("get_context ok")
            .expect("context present");
        assert_eq!(ctx.application_version.as_deref(), Some("2.1.0"));
    }

    // A missing ApplicationMeta row leaves application_version None (not an error).
    #[test]
    fn get_context_without_app_meta_has_no_version() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let app_key = key::ApplicationMeta::new(ApplicationId::from([0xBB; 32]));
        let cid = ContextId::from([0x08; 32]);
        let ctx_meta = types::ContextMeta::new(app_key, [0u8; 32], vec![], None);
        {
            let mut handle = store.handle();
            handle
                .put(&key::ContextMeta::new(cid), &ctx_meta)
                .expect("seed ctx meta");
        }

        let registry = ContextRegistry::new(store.clone());
        let ctx = registry
            .get_context(&cid)
            .expect("get_context ok")
            .expect("context present");
        assert_eq!(ctx.application_version, None);
    }

    // get_context resolves the human-readable name from the owning group's
    // per-context metadata record; absent rows leave it None.
    #[test]
    fn get_context_carries_group_metadata_name() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let app_key = key::ApplicationMeta::new(ApplicationId::from([0xCC; 32]));
        let cid = ContextId::from([0x09; 32]);
        let gid = [0x42u8; 32];
        let ctx_meta = types::ContextMeta::new(app_key, [0u8; 32], vec![], None);
        {
            let mut handle = store.handle();
            handle
                .put(&key::ContextMeta::new(cid), &ctx_meta)
                .expect("seed ctx meta");
            handle
                .put(&key::ContextGroupRef::new(cid.into()), &gid)
                .expect("seed group ref");
            handle
                .put(
                    &key::GroupContextMetadata::new(gid, cid.into()),
                    &calimero_primitives::metadata::MetadataRecord {
                        name: Some("docs-workspace".to_owned()),
                        ..Default::default()
                    },
                )
                .expect("seed context metadata");
        }

        let registry = ContextRegistry::new(store.clone());
        let ctx = registry
            .get_context(&cid)
            .expect("get_context ok")
            .expect("context present");
        assert_eq!(ctx.name.as_deref(), Some("docs-workspace"));

        // No metadata record → name stays None.
        let bare = ContextId::from([0x0A; 32]);
        {
            let mut handle = store.handle();
            handle
                .put(
                    &key::ContextMeta::new(bare),
                    &types::ContextMeta::new(app_key, [0u8; 32], vec![], None),
                )
                .expect("seed bare ctx meta");
        }
        let ctx = registry
            .get_context(&bare)
            .expect("get_context ok")
            .expect("context present");
        assert_eq!(ctx.name, None);
    }
}
