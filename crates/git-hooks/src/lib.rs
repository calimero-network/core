//! Dev-only helper crate.
//!
//! All of the work lives in `build.rs`, which installs the scripts under
//! `.githooks/` into the directory git uses for hooks in this checkout. It is
//! worktree-aware: rather than guessing the `.git` location, it asks git via
//! `git rev-parse --git-path hooks`, which resolves correctly for plain clones,
//! linked worktrees (where `.git` is a file and hooks live in the common dir),
//! and any `core.hooksPath` override.
//!
//! The library itself is intentionally empty; it exists only so the build
//! script runs as part of a normal `cargo build`/`cargo test`.
