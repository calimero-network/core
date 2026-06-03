//! HashComparison sync types (CIP §4 - State Machine, STATE-BASED branch).
//!
//! Types for Merkle tree traversal and hash-based synchronization.

use std::collections::HashSet;

use borsh::{BorshDeserialize, BorshSerialize};

// Re-export the unified CrdtType from primitives (consolidated per issue #1912)
pub use calimero_primitives::crdt::CrdtType;

// =============================================================================
// Constants
// =============================================================================

/// Maximum nodes per response to prevent memory exhaustion.
///
/// Limits the size of `TreeNodeResponse::nodes` to prevent DoS attacks
/// from malicious peers sending oversized responses.
pub const MAX_NODES_PER_RESPONSE: usize = 1000;

/// Maximum children per node (typical Merkle trees use binary or small fanout).
///
/// This limit prevents memory exhaustion from malicious nodes with excessive children.
pub const MAX_CHILDREN_PER_NODE: usize = 256;

/// Maximum size for leaf value data (1 MB).
///
/// Prevents memory exhaustion from malicious peers sending oversized leaf values.
/// This should be sufficient for most entity data while protecting against DoS.
pub const MAX_LEAF_VALUE_SIZE: usize = 1_048_576;

/// Maximum allowed tree depth for traversal requests.
///
/// This limit prevents resource exhaustion from malicious peers requesting
/// extremely deep traversals. Most practical Merkle trees have depth < 32.
pub const MAX_TREE_DEPTH: usize = 64;

// =============================================================================
// Tree Node Request/Response
// =============================================================================

/// Request to traverse the Merkle tree for hash comparison.
///
/// Used for recursive tree traversal to identify differing entities.
/// Start at root, request children, compare hashes, recurse on differences.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct TreeNodeRequest {
    /// ID of the node to request (root hash or internal node hash).
    pub node_id: [u8; 32],

    /// Maximum depth to traverse from this node.
    ///
    /// This field is private to enforce validation. Use constructors and accessors:
    /// - `new()` / `with_depth()` - create requests with validated depth
    /// - `depth()` - get clamped value (always <= MAX_TREE_DEPTH)
    ///
    /// This prevents DoS attacks where an attacker sends a request with an
    /// extremely large depth value to cause resource exhaustion.
    max_depth: Option<usize>,
}

impl TreeNodeRequest {
    /// Create a request for a specific node.
    #[must_use]
    pub fn new(node_id: [u8; 32]) -> Self {
        Self {
            node_id,
            max_depth: None,
        }
    }

    /// Create a request with depth limit.
    #[must_use]
    pub fn with_depth(node_id: [u8; 32], max_depth: usize) -> Self {
        Self {
            node_id,
            // Clamp to MAX_TREE_DEPTH to prevent resource exhaustion
            max_depth: Some(max_depth.min(MAX_TREE_DEPTH)),
        }
    }

    /// Create a request for the root node.
    #[must_use]
    pub fn root(root_hash: [u8; 32]) -> Self {
        Self::new(root_hash)
    }

    /// Get the validated depth limit.
    ///
    /// Always clamps to MAX_TREE_DEPTH, even if raw field was set to a larger
    /// value (e.g., via deserialization from an untrusted source).
    ///
    /// Use this instead of accessing `max_depth` directly when processing requests.
    #[must_use]
    pub fn depth(&self) -> Option<usize> {
        self.max_depth.map(|d| d.min(MAX_TREE_DEPTH))
    }
}

/// Response containing tree nodes for hash comparison.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct TreeNodeResponse {
    /// Nodes in the requested subtree.
    ///
    /// Limited to MAX_NODES_PER_RESPONSE entries. Use `is_valid()` to check
    /// bounds after deserialization from untrusted sources.
    pub nodes: Vec<TreeNode>,

    /// True if the requested node was not found.
    pub not_found: bool,
}

impl TreeNodeResponse {
    /// Create a response with nodes.
    #[must_use]
    pub fn new(nodes: Vec<TreeNode>) -> Self {
        Self {
            nodes,
            not_found: false,
        }
    }

    /// Create a not-found response.
    #[must_use]
    pub fn not_found() -> Self {
        Self {
            nodes: vec![],
            not_found: true,
        }
    }

    /// Check if response contains any leaf nodes.
    #[must_use]
    pub fn has_leaves(&self) -> bool {
        self.nodes.iter().any(|n| n.is_leaf())
    }

    /// Get an iterator over leaf nodes in response.
    ///
    /// Returns an iterator rather than allocating a Vec, which is more
    /// efficient for single-pass iteration.
    pub fn leaves(&self) -> impl Iterator<Item = &TreeNode> {
        self.nodes.iter().filter(|n| n.is_leaf())
    }

    /// Check if response is within valid bounds.
    ///
    /// Call this after deserializing from untrusted sources to prevent
    /// memory exhaustion attacks. Validates both response size and all
    /// contained nodes.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.nodes.len() <= MAX_NODES_PER_RESPONSE && self.nodes.iter().all(TreeNode::is_valid)
    }
}

// =============================================================================
// Tree Node
// =============================================================================

/// A node in the Merkle tree.
///
/// Can be either an internal node (has children) or a leaf node (has data).
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct TreeNode {
    /// Node ID - stable identifier derived from the node's position/key in the tree.
    ///
    /// For internal nodes: typically hash of path or concatenation of child keys.
    /// For leaf nodes: typically hash of the entity key.
    /// This ID remains stable even when content changes.
    pub id: [u8; 32],

    /// Merkle hash - changes when subtree content changes.
    ///
    /// For internal nodes: hash of all children's hashes (propagates changes up).
    /// For leaf nodes: hash of the leaf data (key + value + metadata).
    /// Used for efficient comparison: if hashes match, subtrees are identical.
    pub hash: [u8; 32],

    /// Child node IDs (empty for leaf nodes).
    ///
    /// Typically limited to MAX_CHILDREN_PER_NODE. Use `is_valid()` to check
    /// bounds after deserialization from untrusted sources.
    pub children: Vec<[u8; 32]>,

    /// Leaf data (present only for leaf nodes).
    pub leaf_data: Option<TreeLeafData>,

    /// Tombstones for children removed from this (internal) node.
    ///
    /// A deletion is the absence of a child from `children`, which the
    /// add-biased comparison can't observe. Carrying the tombstones here lets a
    /// peer that still holds a child converge to the deletion (delete-wins)
    /// during comparison — without anyone pushing the live entity. Empty for
    /// leaf nodes. Populated by `get_local_tree_node` from the parent index's
    /// `deleted_children` (each entry resolved to a signed `EntityDeletion`).
    pub deleted_children: Vec<EntityDeletion>,
}

impl TreeNode {
    /// Create an internal node.
    #[must_use]
    pub fn internal(id: [u8; 32], hash: [u8; 32], children: Vec<[u8; 32]>) -> Self {
        Self {
            id,
            hash,
            children,
            leaf_data: None,
            deleted_children: Vec::new(),
        }
    }

    /// Create a leaf node.
    #[must_use]
    pub fn leaf(id: [u8; 32], hash: [u8; 32], data: TreeLeafData) -> Self {
        Self {
            id,
            hash,
            children: vec![],
            leaf_data: Some(data),
            deleted_children: Vec::new(),
        }
    }

    /// Check if node is within valid bounds and structurally valid.
    ///
    /// Call this after deserializing from untrusted sources.
    /// Validates:
    /// - Children count within MAX_CHILDREN_PER_NODE
    /// - Structural invariant: must have exactly one of children OR leaf_data
    /// - Leaf data validity (value size within limits)
    #[must_use]
    pub fn is_valid(&self) -> bool {
        // Check children count
        if self.children.len() > MAX_CHILDREN_PER_NODE {
            return false;
        }

        // Check structural invariant: must be exactly one of internal or leaf.
        // - Internal node: has live children OR only-tombstoned children
        //   (a parent cleared to childless still carries `deleted_children`),
        //   and no leaf_data.
        // - Leaf node: has leaf_data, no children, no deleted_children.
        let is_internal = !self.children.is_empty() || !self.deleted_children.is_empty();
        let is_leaf = self.leaf_data.is_some();
        if is_internal == is_leaf {
            // Invalid: either both (ambiguous) or neither (empty).
            return false;
        }

        // Validate leaf data if present
        if let Some(ref leaf_data) = self.leaf_data {
            if !leaf_data.is_valid() {
                return false;
            }
        }

        true
    }

    /// Check if this is a leaf node.
    #[must_use]
    pub fn is_leaf(&self) -> bool {
        self.leaf_data.is_some()
    }

    /// Check if this is an internal node.
    #[must_use]
    pub fn is_internal(&self) -> bool {
        self.leaf_data.is_none()
    }

    /// Get number of children (0 for leaf nodes).
    #[must_use]
    pub fn child_count(&self) -> usize {
        self.children.len()
    }
}

// =============================================================================
// Tree Leaf Data
// =============================================================================

/// Data stored at a leaf node (entity).
///
/// Contains ALL information needed for CRDT merge on the receiving side.
/// CRITICAL: `metadata` MUST include `crdt_type` for proper merge.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct TreeLeafData {
    /// Entity key (unique identifier within collection).
    pub key: [u8; 32],

    /// Serialized entity value.
    pub value: Vec<u8>,

    /// Entity metadata including crdt_type.
    /// CRITICAL: Must be included for CRDT merge to work correctly.
    pub metadata: LeafMetadata,
}

impl TreeLeafData {
    /// Create leaf data.
    #[must_use]
    pub fn new(key: [u8; 32], value: Vec<u8>, metadata: LeafMetadata) -> Self {
        Self {
            key,
            value,
            metadata,
        }
    }

    /// Check if leaf data is within valid bounds.
    ///
    /// Call this after deserializing from untrusted sources.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.value.len() <= MAX_LEAF_VALUE_SIZE
    }
}

/// Metadata for a leaf entity.
///
/// Minimal metadata needed for CRDT merge during sync.
///
/// This is a wire-protocol-optimized subset of `calimero_storage::Metadata`.
/// It contains only the fields needed for sync operations, avoiding larger
/// fields like `field_name: String` that aren't needed over the wire.
///
/// When receiving entities, implementations should map this to/from the
/// storage layer's `Metadata` type.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct LeafMetadata {
    /// CRDT type for proper merge semantics.
    pub crdt_type: CrdtType,

    /// HLC timestamp of last modification (maps to `Metadata::updated_at`).
    pub hlc_timestamp: u64,

    /// Entity creation timestamp (maps to `Metadata::created_at`).
    ///
    /// **Must** be carried over the wire: `ChildInfo` orders a parent's
    /// children by `created_at` (then `id`), and that order feeds the
    /// parent's — and ultimately the root's — Merkle hash. If a node that
    /// received an entity via HashComparison repair stamped it with
    /// `created_at = 0` (the old behaviour) while another node has the
    /// originating `created_at` from delta-apply, the two compute a
    /// different root hash for identical logical state — the
    /// "Same DAG heads but different root hash" divergence in #2319 — and
    /// HashComparison can never heal it because the repair itself
    /// reintroduces the mismatch. Defaults to `0` only for legacy/test
    /// constructions that don't know it; production builders set it from
    /// the entity's stored `Metadata::created_at`.
    pub created_at: u64,

    /// Version counter (for some CRDT types).
    pub version: u64,

    /// Collection ID this entity belongs to.
    pub collection_id: [u8; 32],

    /// Optional parent entity ID (for nested structures).
    ///
    /// Kept for backward compatibility with peers that ship only the
    /// immediate parent; new senders also populate `ancestors` below so
    /// the receiver can rebuild the entire chain at once.
    pub parent_id: Option<[u8; 32]>,

    /// Full ancestor chain from immediate parent up to (but not
    /// including) the system root, ordered immediate-parent first.
    /// Mirrors what `calimero_storage::index::Index::get_ancestors_of`
    /// returns on the sender — see `LeafMetadata::with_ancestors`.
    ///
    /// Populated by the HashComparison and LevelWise sender paths.
    /// Empty means "this peer didn't ship a chain" — `apply_leaf_with_crdt_merge`
    /// then falls back to the single-`parent_id` reconstruction path.
    ///
    /// Without this field, when a receiver gets a leaf whose immediate
    /// parent doesn't have a local index entry yet (BFS-vs-DFS batch
    /// ordering in EntityPush), `apply_action`'s ancestor loop creates
    /// the parent via `Index::add_root(parent_clone)` — i.e. as a
    /// top-level root entity, parent_id=None. That's the wrong tree
    /// position; the receiver's Merkle hash for the misplaced ancestor
    /// then diverges from the sender's, and HashComparison can't heal
    /// the divergence because every repair attempt reintroduces the
    /// same misplacement. With this field, the loop sees the full
    /// `[parent, grandparent, …, root_child]` and `add_child_to`s each
    /// to the next up — correct topology.
    pub ancestors: Vec<calimero_storage::entities::ChildInfo>,

    /// Authorization triple for `Shared` / `User` storage entities —
    /// the access-control list + signature data the receiver needs to
    /// verify the writer's authorization without consulting the
    /// originator's tree state.
    ///
    /// The signed payload (see `Action::payload_for_signing`) commits
    /// to exactly the access-control triple carried here:
    /// type tag + writers/owner + nonce + (optional) signer hint.
    /// All values are reconstructible from this struct + the entity's
    /// `key` and `value` already on the wire, so the receiver verifies
    /// the signature without any tree-state context.
    ///
    /// `None` for `Public` / `Frozen` entities (no signature required),
    /// or for legacy entities written before sync-signature wire
    /// support. Receivers seeing `None` for an entity their local
    /// state holds as `Shared` / `User` must skip the sync apply for
    /// that entity (let the delta-path repair instead) rather than
    /// trying to construct an action without authorization data.
    pub authorization: Option<calimero_storage::entities::StorageType>,
}

impl LeafMetadata {
    /// Create metadata with required fields. `created_at` defaults to
    /// `0`; production builders should chain [`with_created_at`] from the
    /// entity's stored `Metadata::created_at` (see the field docs).
    ///
    /// [`with_created_at`]: LeafMetadata::with_created_at
    #[must_use]
    pub fn new(crdt_type: CrdtType, hlc_timestamp: u64, collection_id: [u8; 32]) -> Self {
        Self {
            crdt_type,
            hlc_timestamp,
            created_at: 0,
            version: 0,
            collection_id,
            parent_id: None,
            ancestors: Vec::new(),
            authorization: None,
        }
    }

    /// Set the storage-type authorization triple for `Shared` / `User`
    /// entities so the receiver can verify the writer's signature.
    /// See the field doc on `authorization`.
    ///
    /// Only `Shared` and `User` storage types carry authorization; for
    /// `Public` / `Frozen` the wire field must stay `None` (the receiver
    /// has no signature to verify on those, and applying a wire-supplied
    /// `Public` here would open a storage-type-downgrade path on the
    /// receiver). Wrong-type calls log a `warn!` and leave
    /// `authorization` unset in both debug and release — uniform
    /// behavior. Callers should go through
    /// `crate::sync::helpers::wire_authorization_for` in `calimero-node`
    /// rather than building this directly; the helper is the single
    /// source of truth for which storage types carry wire authorization.
    /// This guard is defense-in-depth in case a future caller bypasses
    /// the helper.
    #[must_use]
    pub fn with_authorization(
        mut self,
        authorization: calimero_storage::entities::StorageType,
    ) -> Self {
        use calimero_storage::entities::StorageType;
        let is_auth_type = matches!(
            authorization,
            StorageType::Shared { .. }
                | StorageType::User { .. }
                | StorageType::SharedMember { .. }
        );
        if is_auth_type {
            self.authorization = Some(authorization);
        } else {
            tracing::warn!(
                bad_type = ?authorization,
                "with_authorization called with non-Shared/User storage type — \
                 ignoring; this is a programming error. Callers should use \
                 `calimero_node::sync::helpers::wire_authorization_for` which \
                 only returns Shared/User."
            );
        }
        self
    }

    /// Set the entity creation timestamp (`Metadata::created_at`).
    #[must_use]
    pub fn with_created_at(mut self, created_at: u64) -> Self {
        self.created_at = created_at;
        self
    }

    /// Set version.
    #[must_use]
    pub fn with_version(mut self, version: u64) -> Self {
        self.version = version;
        self
    }

    /// Set parent ID.
    #[must_use]
    pub fn with_parent(mut self, parent_id: [u8; 32]) -> Self {
        self.parent_id = Some(parent_id);
        self
    }

    /// Set the full ancestor chain (immediate parent first, root_child
    /// last; root itself excluded — matches `Index::get_ancestors_of`).
    ///
    /// HC and LevelWise senders populate this so the receiver's
    /// `apply_action` ancestor loop has the chain it needs to attach
    /// each missing ancestor to its real parent instead of falling
    /// back to `add_root` (which places intermediate ancestors at the
    /// wrong tree level, diverging the Merkle root — see field doc).
    #[must_use]
    pub fn with_ancestors(mut self, ancestors: Vec<calimero_storage::entities::ChildInfo>) -> Self {
        self.ancestors = ancestors;
        self
    }
}

// =============================================================================
// Tree Compare Result
// =============================================================================

/// Result of comparing two tree nodes.
///
/// Used for Merkle tree traversal during HashComparison sync.
/// Identifies which children need further traversal in both directions.
///
/// Note: Borsh derives are included for consistency with other sync types and
/// potential future use in batched comparison responses over the wire.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub enum TreeCompareResult {
    /// Hashes match - no sync needed for this subtree.
    Equal,
    /// Hashes differ - need to recurse or fetch leaf.
    ///
    /// For internal nodes: lists children to recurse into.
    /// For leaf nodes: all vecs will be empty, but Different still indicates
    /// that the leaf data needs to be fetched and merged bidirectionally.
    Different {
        /// IDs of children in remote but not in local (need to fetch).
        remote_only_children: Vec<[u8; 32]>,
        /// IDs of children in local but not in remote (for bidirectional sync).
        local_only_children: Vec<[u8; 32]>,
        /// IDs of children present on both sides that need recursive comparison.
        /// These are the primary candidates for recursion when parent hashes differ.
        common_children: Vec<[u8; 32]>,
    },
    /// Local node missing - need to fetch from remote.
    LocalMissing,
    /// Remote node missing - local has data that remote doesn't.
    /// For bidirectional sync, this means we may need to push to remote.
    RemoteMissing,
}

impl TreeCompareResult {
    /// Check if sync (pull from remote) is needed.
    ///
    /// Returns true if local needs data from remote.
    #[must_use]
    pub fn needs_sync(&self) -> bool {
        !matches!(self, Self::Equal | Self::RemoteMissing)
    }

    /// Check if push (send to remote) is needed for bidirectional sync.
    ///
    /// Returns true if local has data that remote doesn't:
    /// - `RemoteMissing`: entire local subtree needs pushing
    /// - `Different` with `local_only_children`: those children need pushing
    /// - `Different` with all empty vecs: this is a **leaf node comparison** where
    ///   hashes differ, meaning local leaf data needs pushing for CRDT merge
    #[must_use]
    pub fn needs_push(&self) -> bool {
        match self {
            Self::RemoteMissing => true,
            Self::Different {
                remote_only_children,
                local_only_children,
                common_children,
            } => {
                // Push needed if we have local-only children
                if !local_only_children.is_empty() {
                    return true;
                }
                // Leaf node detection: when all child vecs are empty but hashes differed,
                // we compared two leaf nodes with different content. The local leaf data
                // needs to be pushed for bidirectional CRDT merge.
                remote_only_children.is_empty() && common_children.is_empty()
            }
            _ => false,
        }
    }
}

// =============================================================================
// Compare Function
// =============================================================================

/// Compare local and remote tree nodes.
///
/// Returns which children (if any) need further traversal in both directions.
/// This supports bidirectional sync where both nodes may have unique data.
///
/// # Arguments
/// * `local` - Local tree node, or None if not present locally
/// * `remote` - Remote tree node, or None if not present on remote
///
/// # Precondition
/// A tombstone propagated during HashComparison bidirectional sync.
///
/// HashComparison reconciles entity trees by add-wins union: a child present on
/// one side but missing on the other is otherwise re-added to the side that
/// lacks it. A node that deleted (cleared) a child must therefore propagate the
/// deletion explicitly, or a peer that still holds it never converges (the
/// clear split-brain). Each deletion carries the tombstone `metadata` so the
/// receiver applies it through the same authenticated `Action::DeleteRef` path
/// as the delta stream — signature/nonce verified for `User`/`Shared` entities,
/// and delete-wins by HLC (`deleted_at` vs the local entity's `updated_at`).
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct EntityDeletion {
    /// Id of the deleted entity.
    pub id: [u8; 32],
    /// HLC timestamp at which the deletion occurred (the tombstone nonce).
    pub deleted_at: u64,
    /// Tombstone metadata, carrying the storage type + signature needed to
    /// authenticate the deletion on the receiver.
    pub metadata: calimero_storage::entities::Metadata,
}

/// When both nodes are present, they must represent the same tree position
/// (i.e., have matching IDs). Comparing nodes at different positions is a
/// caller bug and will trigger a debug assertion.
///
/// # Returns
/// * `Equal` - Hashes match, no sync needed
/// * `Different` - Hashes differ, contains children needing traversal
/// * `LocalMissing` - Need to fetch from remote
/// * `RemoteMissing` - Local has data remote doesn't (for bidirectional push)
#[must_use]
pub fn compare_tree_nodes(
    local: Option<&TreeNode>,
    remote: Option<&TreeNode>,
) -> TreeCompareResult {
    match (local, remote) {
        (None, None) => TreeCompareResult::Equal,
        (None, Some(_)) => TreeCompareResult::LocalMissing,
        (Some(_), None) => TreeCompareResult::RemoteMissing,
        (Some(local_node), Some(remote_node)) => {
            // Verify precondition: nodes must represent the same tree position
            debug_assert_eq!(
                local_node.id, remote_node.id,
                "compare_tree_nodes called with nodes at different tree positions"
            );

            if local_node.hash == remote_node.hash {
                TreeCompareResult::Equal
            } else {
                // Use HashSet for O(1) lookups instead of O(n) Vec::contains
                let local_children: HashSet<&[u8; 32]> = local_node.children.iter().collect();
                let remote_children: HashSet<&[u8; 32]> = remote_node.children.iter().collect();

                // Children in remote but not in local (need to fetch)
                let remote_only_children: Vec<[u8; 32]> = remote_node
                    .children
                    .iter()
                    .filter(|child_id| !local_children.contains(child_id))
                    .copied()
                    .collect();

                // Children in local but not in remote (for bidirectional sync)
                let local_only_children: Vec<[u8; 32]> = local_node
                    .children
                    .iter()
                    .filter(|child_id| !remote_children.contains(child_id))
                    .copied()
                    .collect();

                // Children present on both sides - these are the primary candidates
                // for recursive comparison when parent hashes differ
                let common_children: Vec<[u8; 32]> = local_node
                    .children
                    .iter()
                    .filter(|child_id| remote_children.contains(child_id))
                    .copied()
                    .collect();

                TreeCompareResult::Different {
                    remote_only_children,
                    local_only_children,
                    common_children,
                }
            }
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tree_node_request_roundtrip() {
        let request = TreeNodeRequest::with_depth([1; 32], 3);

        let encoded = borsh::to_vec(&request).expect("serialize");
        let decoded: TreeNodeRequest = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(request, decoded);
        assert_eq!(decoded.max_depth, Some(3));
    }

    #[test]
    fn test_tree_node_request_root() {
        let root_hash = [42; 32];
        let request = TreeNodeRequest::root(root_hash);

        assert_eq!(request.node_id, root_hash);
        assert!(request.max_depth.is_none());
    }

    #[test]
    fn test_tree_node_internal() {
        let node = TreeNode::internal([1; 32], [2; 32], vec![[3; 32], [4; 32]]);

        assert!(node.is_internal());
        assert!(!node.is_leaf());
        assert_eq!(node.child_count(), 2);
        assert!(node.leaf_data.is_none());
    }

    #[test]
    fn test_tree_node_leaf() {
        let metadata = LeafMetadata::new(CrdtType::lww_register("test"), 12345, [5; 32]);
        let leaf_data = TreeLeafData::new([1; 32], vec![1, 2, 3], metadata);
        let node = TreeNode::leaf([2; 32], [3; 32], leaf_data);

        assert!(node.is_leaf());
        assert!(!node.is_internal());
        assert_eq!(node.child_count(), 0);
        assert!(node.leaf_data.is_some());
    }

    #[test]
    fn test_tree_node_opaque_leaf_is_valid() {
        // An "opaque" Merkle leaf (no stored crdt_type) is carried on the wire as
        // a leaf with a synthetic `LwwRegister { inner_type: "Opaque" }` type;
        // such a node must be structurally valid so the receiving peer does not
        // drop it as "Invalid TreeNode".
        let metadata =
            LeafMetadata::new(CrdtType::lww_register("Opaque"), 42, [0u8; 32]).with_created_at(7);
        let leaf_data = TreeLeafData::new([118u8; 32], b"app-root-state".to_vec(), metadata);
        let node = TreeNode::leaf([118u8; 32], [9u8; 32], leaf_data);

        assert!(node.is_leaf());
        assert!(!node.is_internal());
        assert!(node.is_valid(), "opaque leaf node must be valid");
    }

    #[test]
    fn test_tree_node_roundtrip() {
        let metadata = LeafMetadata::new(CrdtType::unordered_map("String", "u64"), 999, [6; 32])
            .with_version(5)
            .with_parent([7; 32]);
        let leaf_data = TreeLeafData::new([1; 32], vec![4, 5, 6], metadata);
        let node = TreeNode::leaf([2; 32], [3; 32], leaf_data);

        let encoded = borsh::to_vec(&node).expect("serialize");
        let decoded: TreeNode = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(node, decoded);
    }

    #[test]
    fn test_tree_node_response_roundtrip() {
        let internal = TreeNode::internal([1; 32], [2; 32], vec![[3; 32]]);
        let metadata = LeafMetadata::new(CrdtType::Rga, 100, [4; 32]);
        let leaf_data = TreeLeafData::new([5; 32], vec![7, 8, 9], metadata);
        let leaf = TreeNode::leaf([6; 32], [7; 32], leaf_data);

        let response = TreeNodeResponse::new(vec![internal, leaf]);

        let encoded = borsh::to_vec(&response).expect("serialize");
        let decoded: TreeNodeResponse = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(response, decoded);
        assert!(decoded.has_leaves());
        assert_eq!(decoded.leaves().count(), 1);
    }

    #[test]
    fn test_tree_node_response_not_found() {
        let response = TreeNodeResponse::not_found();

        assert!(response.not_found);
        assert!(response.nodes.is_empty());
        assert!(!response.has_leaves());
    }

    #[test]
    fn test_leaf_metadata_builder() {
        let metadata = LeafMetadata::new(CrdtType::PnCounter, 500, [1; 32])
            .with_version(10)
            .with_parent([2; 32]);

        assert_eq!(metadata.crdt_type, CrdtType::PnCounter);
        assert_eq!(metadata.hlc_timestamp, 500);
        assert_eq!(metadata.version, 10);
        assert_eq!(metadata.parent_id, Some([2; 32]));
    }

    #[test]
    fn test_crdt_type_variants() {
        let types = vec![
            CrdtType::lww_register("test"),
            CrdtType::GCounter,
            CrdtType::PnCounter,
            CrdtType::Rga,
            CrdtType::unordered_map("String", "u64"),
            CrdtType::unordered_set("String"),
            CrdtType::vector("u64"),
            CrdtType::UserStorage,
            CrdtType::FrozenStorage,
            CrdtType::Custom("test".to_string()),
        ];

        for crdt_type in types {
            let encoded = borsh::to_vec(&crdt_type).expect("serialize");
            let decoded: CrdtType = borsh::from_slice(&encoded).expect("deserialize");
            assert_eq!(crdt_type, decoded);
        }
    }

    #[test]
    fn test_compare_tree_nodes_equal() {
        let local = TreeNode::internal([1; 32], [99; 32], vec![[2; 32]]);
        let remote = TreeNode::internal([1; 32], [99; 32], vec![[2; 32]]);

        let result = compare_tree_nodes(Some(&local), Some(&remote));
        assert_eq!(result, TreeCompareResult::Equal);
        assert!(!result.needs_sync());
    }

    #[test]
    fn test_compare_tree_nodes_local_missing() {
        let remote = TreeNode::internal([1; 32], [2; 32], vec![[3; 32]]);

        let result = compare_tree_nodes(None, Some(&remote));
        assert_eq!(result, TreeCompareResult::LocalMissing);
        assert!(result.needs_sync());
    }

    #[test]
    fn test_compare_tree_nodes_different() {
        let local = TreeNode::internal([1; 32], [10; 32], vec![[2; 32]]);
        let remote = TreeNode::internal([1; 32], [20; 32], vec![[2; 32], [3; 32]]);

        let result = compare_tree_nodes(Some(&local), Some(&remote));

        match &result {
            TreeCompareResult::Different {
                remote_only_children,
                local_only_children: _,
                common_children,
            } => {
                // [3; 32] is in remote but not in local
                assert!(remote_only_children.contains(&[3; 32]));
                // [2; 32] is common to both sides
                assert!(common_children.contains(&[2; 32]));
            }
            _ => panic!("Expected Different result"),
        }
        assert!(result.needs_sync());
    }

    #[test]
    fn test_tree_compare_result_needs_sync() {
        assert!(!TreeCompareResult::Equal.needs_sync());
        assert!(!TreeCompareResult::RemoteMissing.needs_sync());
        assert!(TreeCompareResult::LocalMissing.needs_sync());
        assert!(TreeCompareResult::Different {
            remote_only_children: vec![],
            local_only_children: vec![],
            common_children: vec![],
        }
        .needs_sync());
    }

    #[test]
    fn test_tree_compare_result_roundtrip() {
        let variants = vec![
            TreeCompareResult::Equal,
            TreeCompareResult::LocalMissing,
            TreeCompareResult::RemoteMissing,
            TreeCompareResult::Different {
                remote_only_children: vec![[1; 32], [2; 32]],
                local_only_children: vec![[3; 32]],
                common_children: vec![[4; 32], [5; 32], [6; 32]],
            },
            TreeCompareResult::Different {
                remote_only_children: vec![],
                local_only_children: vec![],
                common_children: vec![],
            },
        ];

        for original in variants {
            let encoded = borsh::to_vec(&original).expect("encode");
            let decoded: TreeCompareResult = borsh::from_slice(&encoded).expect("decode");
            assert_eq!(original, decoded);
        }
    }

    #[test]
    fn test_compare_tree_nodes_leaf_content_differs() {
        let local_metadata = LeafMetadata::new(CrdtType::lww_register("test"), 100, [1; 32]);
        let local_leaf = TreeLeafData::new([10; 32], vec![1, 2, 3], local_metadata);
        let local = TreeNode::leaf([1; 32], [100; 32], local_leaf);

        let remote_metadata = LeafMetadata::new(CrdtType::lww_register("test"), 200, [1; 32]);
        let remote_leaf = TreeLeafData::new([10; 32], vec![4, 5, 6], remote_metadata);
        let remote = TreeNode::leaf([1; 32], [200; 32], remote_leaf);

        let result = compare_tree_nodes(Some(&local), Some(&remote));

        match &result {
            TreeCompareResult::Different {
                remote_only_children,
                local_only_children,
                common_children,
            } => {
                assert!(remote_only_children.is_empty());
                assert!(local_only_children.is_empty());
                assert!(common_children.is_empty());
            }
            _ => panic!("Expected Different result for leaves with different content"),
        }
        assert!(result.needs_sync());
        assert!(result.needs_push());
    }

    #[test]
    fn test_compare_tree_nodes_remote_missing() {
        let local = TreeNode::internal([1; 32], [2; 32], vec![[3; 32]]);

        let result = compare_tree_nodes(Some(&local), None);
        assert_eq!(result, TreeCompareResult::RemoteMissing);
        assert!(!result.needs_sync());
    }

    #[test]
    fn test_compare_tree_nodes_local_only_children() {
        let local = TreeNode::internal([1; 32], [10; 32], vec![[2; 32], [3; 32], [4; 32]]);
        let remote = TreeNode::internal([1; 32], [20; 32], vec![[2; 32], [5; 32]]);

        let result = compare_tree_nodes(Some(&local), Some(&remote));

        match &result {
            TreeCompareResult::Different {
                remote_only_children,
                local_only_children,
                common_children,
            } => {
                assert!(remote_only_children.contains(&[5; 32]));
                assert!(local_only_children.contains(&[3; 32]));
                assert!(local_only_children.contains(&[4; 32]));
                assert!(common_children.contains(&[2; 32]));
            }
            _ => panic!("Expected Different result"),
        }
    }

    #[test]
    fn test_tree_node_request_max_depth_validation() {
        // Constructors should clamp to MAX_TREE_DEPTH
        let request = TreeNodeRequest::with_depth([1; 32], MAX_TREE_DEPTH);
        assert_eq!(request.depth(), Some(MAX_TREE_DEPTH));

        // Excessive depth should be clamped by constructor
        let excessive = TreeNodeRequest::with_depth([1; 32], MAX_TREE_DEPTH + 100);
        assert_eq!(excessive.depth(), Some(MAX_TREE_DEPTH));
    }

    #[test]
    fn test_tree_node_request_depth_accessor() {
        // depth() returns None when no depth is set
        let request_none = TreeNodeRequest::new([1; 32]);
        assert_eq!(request_none.depth(), None);

        // depth() returns Some with clamped value when set
        let request_with_depth = TreeNodeRequest::with_depth([1; 32], 5);
        assert_eq!(request_with_depth.depth(), Some(5));
    }

    #[test]
    fn test_tree_node_request_depth_clamping_on_deserialize() {
        // Simulate an attacker sending a malicious request with excessive depth
        // by manually constructing the serialized bytes with a huge max_depth value
        let node_id = [1u8; 32];
        let malicious_depth: usize = usize::MAX;

        // Create a request and serialize it
        let mut bytes = Vec::new();
        // node_id: 32 bytes
        bytes.extend_from_slice(&node_id);
        // max_depth: Option<usize> - 1 byte tag (Some = 1) + usize (8 bytes on 64-bit)
        bytes.push(1); // Some variant
        bytes.extend_from_slice(&malicious_depth.to_le_bytes());

        // Deserialize the malicious request
        let request: TreeNodeRequest = borsh::from_slice(&bytes).expect("deserialize");

        // The depth() accessor should clamp to MAX_TREE_DEPTH
        assert_eq!(
            request.depth(),
            Some(MAX_TREE_DEPTH),
            "depth() must clamp deserialized values to MAX_TREE_DEPTH"
        );
    }

    #[test]
    fn test_tree_node_request_private_field_enforces_validation() {
        // This test documents that max_depth is private and cannot be set directly
        // The only ways to create a TreeNodeRequest are:
        // 1. TreeNodeRequest::new() - no depth limit
        // 2. TreeNodeRequest::with_depth() - clamped depth limit
        // 3. Deserialization - depth() accessor clamps when read

        // Verify new() creates request without depth limit
        let no_depth = TreeNodeRequest::new([0; 32]);
        assert_eq!(no_depth.depth(), None);

        // Verify with_depth() clamps excessive values
        let clamped = TreeNodeRequest::with_depth([0; 32], 999_999);
        assert_eq!(clamped.depth(), Some(MAX_TREE_DEPTH));

        // Verify reasonable values pass through
        let reasonable = TreeNodeRequest::with_depth([0; 32], 3);
        assert_eq!(reasonable.depth(), Some(3));
    }

    #[test]
    fn test_tree_node_response_validation() {
        let valid_response =
            TreeNodeResponse::new(vec![TreeNode::internal([1; 32], [2; 32], vec![[3; 32]])]);
        assert!(valid_response.is_valid());

        let metadata = LeafMetadata::new(CrdtType::lww_register("test"), 100, [1; 32]);
        let leaf_data = TreeLeafData::new([10; 32], vec![1, 2, 3], metadata);
        let leaf_response =
            TreeNodeResponse::new(vec![TreeNode::leaf([1; 32], [2; 32], leaf_data)]);
        assert!(leaf_response.is_valid());

        let mut nodes = Vec::new();
        for i in 0..MAX_NODES_PER_RESPONSE {
            let id = [i as u8; 32];
            nodes.push(TreeNode::internal(id, id, vec![[0; 32]]));
        }
        let at_limit = TreeNodeResponse::new(nodes);
        assert!(at_limit.is_valid());
    }

    #[test]
    fn test_tree_node_validation() {
        let valid = TreeNode::internal([1; 32], [2; 32], vec![[3; 32], [4; 32]]);
        assert!(valid.is_valid());

        let children: Vec<[u8; 32]> = (0..MAX_CHILDREN_PER_NODE).map(|i| [i as u8; 32]).collect();
        let at_limit = TreeNode::internal([1; 32], [2; 32], children);
        assert!(at_limit.is_valid());

        let over_children: Vec<[u8; 32]> =
            (0..=MAX_CHILDREN_PER_NODE).map(|i| [i as u8; 32]).collect();
        let over_limit = TreeNode::internal([1; 32], [2; 32], over_children);
        assert!(!over_limit.is_valid());

        let metadata = LeafMetadata::new(CrdtType::lww_register("test"), 100, [1; 32]);
        let leaf_data = TreeLeafData::new([10; 32], vec![1, 2, 3], metadata);
        let invalid_node = TreeNode {
            id: [1; 32],
            hash: [2; 32],
            children: vec![[3; 32]],
            deleted_children: vec![],
            leaf_data: Some(leaf_data),
        };
        assert!(!invalid_node.is_valid());

        let valid_metadata = LeafMetadata::new(CrdtType::lww_register("test"), 100, [1; 32]);
        let valid_leaf_data = TreeLeafData::new([10; 32], vec![1, 2, 3], valid_metadata);
        let valid_leaf = TreeNode::leaf([1; 32], [2; 32], valid_leaf_data);
        assert!(valid_leaf.is_valid());

        let empty_node = TreeNode::internal([1; 32], [2; 32], vec![]);
        assert!(!empty_node.is_valid());
    }

    #[test]
    fn test_tree_node_response_validation_over_limit() {
        let mut nodes = Vec::new();
        for i in 0..=MAX_NODES_PER_RESPONSE {
            let id = [i as u8; 32];
            nodes.push(TreeNode::internal(id, id, vec![[0; 32]]));
        }
        let over_limit = TreeNodeResponse::new(nodes);
        assert!(!over_limit.is_valid());

        let over_children: Vec<[u8; 32]> =
            (0..=MAX_CHILDREN_PER_NODE).map(|i| [i as u8; 32]).collect();
        let invalid_node = TreeNode::internal([1; 32], [2; 32], over_children);
        let response_with_invalid = TreeNodeResponse::new(vec![invalid_node]);
        assert!(!response_with_invalid.is_valid());

        let empty_node = TreeNode::internal([1; 32], [2; 32], vec![]);
        let response_with_empty = TreeNodeResponse::new(vec![empty_node]);
        assert!(!response_with_empty.is_valid());
    }

    #[test]
    fn test_tree_leaf_data_validation() {
        let metadata = LeafMetadata::new(CrdtType::lww_register("test"), 100, [1; 32]);

        let valid = TreeLeafData::new([1; 32], vec![1, 2, 3], metadata.clone());
        assert!(valid.is_valid());

        let at_limit_value = vec![0u8; MAX_LEAF_VALUE_SIZE];
        let at_limit = TreeLeafData::new([1; 32], at_limit_value, metadata.clone());
        assert!(at_limit.is_valid());

        let over_limit_value = vec![0u8; MAX_LEAF_VALUE_SIZE + 1];
        let over_limit = TreeLeafData::new([1; 32], over_limit_value, metadata);
        assert!(!over_limit.is_valid());
    }

    #[test]
    fn test_tree_compare_result_needs_push() {
        assert!(TreeCompareResult::RemoteMissing.needs_push());

        let with_local_only = TreeCompareResult::Different {
            remote_only_children: vec![],
            local_only_children: vec![[1; 32]],
            common_children: vec![],
        };
        assert!(with_local_only.needs_push());

        let with_remote_only = TreeCompareResult::Different {
            remote_only_children: vec![[1; 32]],
            local_only_children: vec![],
            common_children: vec![],
        };
        assert!(!with_remote_only.needs_push());

        let with_common_only = TreeCompareResult::Different {
            remote_only_children: vec![],
            local_only_children: vec![],
            common_children: vec![[1; 32]],
        };
        assert!(!with_common_only.needs_push());

        let differing_leaves = TreeCompareResult::Different {
            remote_only_children: vec![],
            local_only_children: vec![],
            common_children: vec![],
        };
        assert!(differing_leaves.needs_push());

        assert!(!TreeCompareResult::Equal.needs_push());
        assert!(!TreeCompareResult::LocalMissing.needs_push());
    }
}
