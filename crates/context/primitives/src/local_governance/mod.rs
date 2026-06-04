//! **Deprecated shim** for the old `local_governance::*` surface.
//!
//! The op data types (`SignedGroupOp`, `SignedNamespaceOp`, the
//! `GroupOp` / `NamespaceOp` / `RootOp` enums, and their borshable
//! sub-types + signing helpers) have moved to the
//! [`calimero_governance_types`] leaf crate so the to-be-extracted
//! `calimero-governance-store` (see #2307) can depend on them without
//! transitively re-acquiring `actix` through this crate.
//!
//! `AckRouter` and its `tokio::broadcast` pub/sub plumbing stay here:
//! it's a runtime primitive, not a data type, and crosses actor
//! mailboxes.
//!
//! Callers currently importing from `calimero_context_client::local_governance::*`
//! should migrate to `calimero_governance_types::*` over the next
//! release cycle. Issue #2479 / epic #2300.

mod ack_router;
pub use ack_router::AckRouter;

// Curated re-exports — explicit symbol list (not a wildcard) so
// the set is reviewable, and a single `#[deprecated]` attribute on
// the `use` item triggers a migration warning at every import site
// that resolves any of these names. Matches exactly the set
// previously re-exported from this module's old `mod.rs`.

#[deprecated(note = "use calimero_governance_types::* directly")]
pub use calimero_governance_types::{
    hash_scoped_group, hash_scoped_namespace, namespace_op_content_hash, namespace_signable_bytes,
    op_content_hash, signable_bytes, EncryptedGroupOp, GovernanceError, GroupOp, GroupTopicMsg,
    KeyEnvelope, KeyRotation, NamespaceOp, NamespaceTopicMsg, OpaqueSkeleton, ReadinessProbe,
    RootOp, SignableGroupOp, SignableNamespaceOp, SignedAck, SignedGroupOp,
    SignedMigrationHeartbeat, SignedNamespaceOp, SignedReadinessBeacon, StoredNamespaceEntry,
    GROUP_GOVERNANCE_SIGN_DOMAIN, NAMESPACE_GOVERNANCE_SIGN_DOMAIN, SIGNED_GROUP_OP_SCHEMA_VERSION,
    SIGNED_NAMESPACE_OP_SCHEMA_VERSION,
};

// `SignableReadinessBeacon` was only exported via `wire::*` in the old
// file; preserve that.
#[deprecated(note = "use calimero_governance_types::wire::* directly")]
pub use calimero_governance_types::wire::SignableReadinessBeacon;

// Preserve the `local_governance::wire` module path itself for callers
// who used the qualified form (e.g. `local_governance::wire::SomeType`).
// Same deprecation nudge as the flat re-exports above.
#[deprecated(note = "use calimero_governance_types::wire directly")]
pub use calimero_governance_types::wire;
