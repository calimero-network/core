//! Every test in the crate, one file per module under test.
//!
//! They live here rather than inside the modules they cover so that reading
//! `authorize.rs` is reading the decision and nothing else. Each file reaches
//! into its subject through `crate::` paths.
//!
//! The views these tests build are synthetic `AclView` values, never a real
//! projection — which is the point of the crate's shape: the decision is a pure
//! function of an already-resolved view, so every rule can be driven directly
//! without folding an op log to reach it.

mod support;

mod authorize;
mod inheritance;
mod inheritance_climb;
