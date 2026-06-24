//! Namespace concerns consolidated from six previously-separate files
//! (`namespace.rs`, `namespace_dag.rs`, `namespace_governance.rs`,
//! `namespace_membership.rs`, `namespace_op_log.rs`,
//! `namespace_retry.rs`).
//!
//! Submodules group by axis of concern, and the public surface below
//! mirrors what `group_store/mod.rs` previously re-exported so callers
//! continue to see the same symbol set at `calimero_context::group_store::*`.
//!
//! Issue #2480 / epic #2300. Mirror of #2306 for the namespace side.
mod core;
mod dag;
mod governance;
mod membership;
mod op_log;
mod retry;

#[cfg(test)]
mod tests;

pub use self::core::MAX_NAMESPACE_DEPTH;

pub use self::core::{
    CascadePayload, NamespaceIdentityRecord, NamespaceRepository, ReparentOutcome,
    ResolvedNamespaceIdentity,
};
pub use self::dag::{NamespaceDagService, NamespaceHead};
pub(crate) use self::governance::classify_report_readiness;
pub use self::governance::{
    apply_received_group_key, apply_signed_namespace_op, apply_signed_namespace_op_at_cut,
    build_group_key_delivery, collect_skeleton_delta_ids_for_group, decrypt_group_op,
    known_namespace_identities, namespace_groups_awaiting_key,
    namespace_groups_with_held_key_buffered_ops, redrive_buffered_ops_for_group,
    retry_encrypted_ops_for_group, sign_and_publish_namespace_op,
    sign_apply_and_publish_namespace_op, ApplyNamespaceOpResult, KeyUnwrapFailure,
    NamespaceGovernance,
};
pub use self::membership::NamespaceMembershipService;
pub use self::op_log::NamespaceOpLogService;
pub use self::retry::NamespaceRetryService;
