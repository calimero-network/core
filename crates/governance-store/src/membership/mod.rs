//! Group-membership concerns consolidated from five previously-separate
//! files (`membership.rs`, `membership_status.rs`, `membership_view.rs`,
//! `membership_policy.rs`, `membership_policy_rules.rs`).
//!
//! Submodules group by axis of concern, and the public surface below
//! mirrors what `group_store/mod.rs` previously re-exported so callers
//! continue to see the same symbol set at `calimero_context::group_store::*`.
//!
//! Issue #2306 / epic #2300.

mod core;
mod policy;
mod policy_rules;
mod status;
mod view;

#[cfg(test)]
mod tests;

pub use self::core::{MembershipPath, MembershipRepository};
pub use self::policy::MembershipPolicy;
pub(crate) use self::status::role_from_invited_role;
pub use self::status::{acl_view_at, MembershipStatus};
pub use self::view::GroupMembershipView;
