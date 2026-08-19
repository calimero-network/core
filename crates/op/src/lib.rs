//! The one **op** envelope for the unified causal log.
//!
//! Every change — a data write, a writer-set rotation, a membership change, an
//! admin/policy change — is the same [`Op`], carried by the generic
//! `CausalDelta<T>` / `DagStore<T>` transport. A scope's state is the
//! deterministic projection of its op-log (see `calimero-projection`); its
//! single [`scope_root`] is the only convergence signal; authorization is one
//! fold over the op's causal cut (see `calimero-authz`).
//!
//! This crate is the small foundation: the op types plus the canonical id and
//! root hashing.
//!
//! # Where things live
//!
//! | Module | What it holds |
//! | --- | --- |
//! | `scope` | [`ScopeId`], the domain everything is scoped to, and its convergence root [`scope_root`] |
//! | `authorship` | [`Authorship`] — the account/device/key triple, and the sentinel for an op nothing can attribute |
//! | `payload` | [`OpPayload`] — every kind of change, and the append-only wire format its tags are pinned to |
//! | `op` | The [`Op`] envelope and [`Op::compute_id`], the hash that decides what is signed |
//!
//! Dependencies run one way — `op` → `payload` → `scope`, `op` → `authorship` —
//! so the wire format cannot come to depend on the envelope, and a variant can
//! be appended without touching the hashing.
//!
//! Every public item is re-exported here, so `calimero_op::OpPayload` keeps
//! working regardless of which module it moved to.

mod authorship;
mod op;
mod payload;
mod scope;

#[cfg(test)]
mod tests;

pub use crate::authorship::Authorship;
pub use crate::op::Op;
pub use crate::payload::OpPayload;
pub use crate::scope::{scope_root, ScopeId};
