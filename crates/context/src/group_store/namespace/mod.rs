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

pub(crate) use self::core::MAX_NAMESPACE_DEPTH;
pub use self::core::{
    CascadePayload, NamespaceIdentityRecord, NamespaceRepository, ReparentOutcome,
    ResolvedNamespaceIdentity,
};
#[allow(deprecated)]
pub use self::core::{
    collect_descendant_groups, collect_subtree_for_cascade, collect_visible_descendant_groups,
    create_recursive_invitations, get_namespace_identity, get_namespace_identity_record,
    get_or_create_namespace_identity, get_or_create_namespace_identity_bundle, get_parent_group,
    is_authorized_for_context_state_op, is_descendant_of, is_read_only_for_context,
    list_child_groups, nest_group, recursive_remove_member, reparent_group, resolve_namespace,
    resolve_namespace_identity, resolve_namespace_identity_record, store_namespace_identity,
    unnest_group,
};
pub use self::dag::{NamespaceDagService, NamespaceHead};
pub use self::governance::{
    apply_signed_namespace_op, collect_skeleton_delta_ids_for_group, sign_and_publish_namespace_op,
    sign_apply_and_publish_namespace_op, ApplyNamespaceOpResult, KeyUnwrapFailure,
    NamespaceGovernance, PendingKeyDelivery,
};
pub(crate) use self::governance::{classify_report_readiness, min_acks_after_local_mutation};
pub use self::membership::NamespaceMembershipService;
pub use self::op_log::NamespaceOpLogService;
pub use self::retry::NamespaceRetryService;
