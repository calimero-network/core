//! Cascade-engine helpers used by the `CascadeTargetApplicationSet` and
//! `CascadeGroupMigrationSet` apply paths (see PR-2 of the namespace-
//! cascade-migration train).
//!
//! The cascade engine fans one signed group op out to every descendant
//! subgroup whose current `app_key` matches the op's `from_app_key`
//! predicate. The fan-out + predicate evaluation is a pure read of the
//! current group tree and meta state; the apply handler (in
//! `crates/context/src/group_store/mod.rs`) calls into [`walk_for_predicate`]
//! to materialize the per-descendant decision before issuing any writes.
//!
//! Keeping the walk in a small module here (rather than inline in
//! `apply_group_op_mutations`) lets the unit tests exercise the predicate
//! and the cycle / depth bounds in isolation without standing up the
//! whole apply pipeline.

mod walk;

pub(crate) use walk::walk_for_predicate;

#[cfg(test)]
mod walk_depth_bound_tests;
#[cfg(test)]
mod walk_predicate_tests;
