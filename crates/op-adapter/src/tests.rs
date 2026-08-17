//! Every test in the crate, one file per module under test.
//!
//! They live here rather than inside the modules they cover so that reading
//! `root.rs` is reading the admin plane's coverage and nothing else. Each file
//! reaches into its subject through `crate::` paths.
//!
//! Two kinds of assertion appear throughout, and the distinction is what the
//! crate exists to prove:
//!
//! - **encoding** — this source op maps to that payload. Cheap, and enough for a
//!   variant whose meaning the payload carries verbatim.
//! - **fold-equivalence** — encode, then fold through `calimero-projection`'s
//!   `ScopeState` and assert the result matches what the legacy per-plane
//!   resolver answers. That is the only assertion that catches a payload which
//!   is shaped right and means something else.

mod support;

mod acl;
mod credential;
mod data;
mod group;
mod root;
