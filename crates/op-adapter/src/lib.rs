//! Transitional adapter that bridges the per-plane operation types onto the
//! unified causal log. It (and the per-plane source types it reads) is deleted
//! once everything runs on the unified [`OpPayload`](calimero_op::OpPayload).
//!
//! **Encoders** map each per-plane operation onto the one `OpPayload`, so we
//! can prove the unified projection faithfully represents the current system
//! across the planes it still bridges: data (`Action` → `Put`/`Delete`),
//! membership (`GroupOp` → `MemberAdded`/`MemberRemoved`), and admin (`RootOp` →
//! `AdminChanged`/`PolicyUpdated`/`SubgroupCreated`/open-join). In-model vs
//! out-of-model coverage is documented per encoder. A cell's writer set has no
//! encoder: it rides governance as `SharedWritersRotated`.
//!
//! The proof of faithfulness is deterministic **fold-equivalence**: the unified
//! projection resolves the same membership as the current resolvers over the
//! same op sequence (the membership-fold property test in
//! `calimero-governance-store`).
//!
//! # Where things live
//!
//! One module per plane, because the planes are what the coverage docs are
//! written against — a reviewer asking "is this `GroupOp` variant folded?" opens
//! exactly one file to answer it.
//!
//! | Module | What it holds |
//! | --- | --- |
//! | `data` | Data plane: [`payload_from_action`] |
//! | `group` | Membership plane: [`payload_from_group_op`] |
//! | `root` | Admin/namespace plane: [`payload_from_root_op`] |
//! | `guard` | Owner-level ops: a `RootGuarded` wrapper and its proof's op-local check |
//! | `credential` | [`join_credential_binds`] / [`join_credential_certifies`] — the op-local admission predicates the apply path shares |
//!
//! Every public item is re-exported here, so `calimero_op_adapter::payload_from_root_op`
//! keeps working regardless of which module it moved to.

mod credential;
mod data;
mod group;
mod guard;
mod root;

#[cfg(test)]
mod tests;

pub use crate::credential::{join_credential_binds, join_credential_certifies};
pub use crate::data::payload_from_action;
pub use crate::group::{payload_from_group_op, payload_from_pre_guard_group_op};
pub use crate::root::{payload_from_pre_guard_root_op, payload_from_root_op};
