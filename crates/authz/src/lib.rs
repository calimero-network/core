//! The **one** authorization fold for the unified causal log.
//!
//! [`authorize`] is the single security boundary: one match over [`OpPayload`](calimero_op::OpPayload)
//! arms against an [`AclView`] resolved at the op's causal cut. It unifies what
//! were three separate causal-auth checks — writer-set resolution, group
//! membership resolution, and the per-delta governance-position gate.
//!
//! **Causal-honor semantics:** an op is authorized against the ACL/membership
//! *as of its own causal parents*, never the receiver's current state. So a
//! write authored before a revocation, in causal order, stays valid regardless
//! of the order a receiver observes the revocation (the forward-only property).
//! The caller produces the [`AclView`] via `ScopeState::acl_view_at(op.parents)`
//! (see `calimero-projection`); this crate is the pure decision over that view.
//!
//! # Where things live
//!
//! | Module | What it holds |
//! | --- | --- |
//! | `authorize` | The decision itself: [`authorize`], [`required_mask_for`], and the device-binding precondition every op but a link must pass |
//! | `view` | [`AclView`] — the at-cut state the decision reads — plus [`AccountBinding`], [`DeviceBinding`], [`SubgroupEdge`] and the flat predicates |
//! | `inheritance` | The subgroup-tree walk shared by three questions, and [`MemberPathAtCut`] |
//! | `admission` | Credential admission: [`AclView::admit_device_link`], [`AclView::admit_key_rotation`], and [`fold_device_link`] — the rules `authorize` and the projection's fold must agree on |
//! | `error` | [`Rejected`] — one rejection type for every plane |
//!
//! Every public item is re-exported here, so `calimero_authz::AclView` keeps
//! working regardless of which module it moved to.

mod admission;
mod authorize;
mod error;
mod inheritance;
mod view;

#[cfg(test)]
mod tests;

pub use crate::admission::fold_device_link;
pub use crate::authorize::{authorize, required_mask_for};
pub use crate::error::Rejected;
pub use crate::inheritance::MemberPathAtCut;
pub use crate::view::{AccountBinding, AclView, DeviceBinding, SubgroupEdge};
