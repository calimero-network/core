//! Storage interface providing functionality for interacting with stored data.
//!
//! This module provides the primary interface for interacting with the storage
//! system, as a layer on top of the underlying database store.
//!

#![forbid(
    unreachable_pub,
    unsafe_code,
    unsafe_op_in_unsafe_fn,
    clippy::missing_docs_in_private_items
)]
#![deny(
    clippy::expect_used,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::panic,
    clippy::unwrap_in_result,
    clippy::unwrap_used
)]
#![warn(
    missing_docs,
    clippy::future_not_send,
    clippy::let_underscore_untyped,
    clippy::map_err_ignore,
    clippy::pattern_type_mismatch,
    clippy::same_name_method,
    clippy::shadow_reuse,
    clippy::shadow_same,
    clippy::shadow_unrelated,
    clippy::unreachable,
    clippy::use_debug
)]
//	Lints specifically disabled for unit tests
#![cfg_attr(
    test,
    allow(
        non_snake_case,
        clippy::arithmetic_side_effects,
        clippy::assigning_clones,
        clippy::cast_lossless,
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cognitive_complexity,
        clippy::default_numeric_fallback,
        clippy::exhaustive_enums,
        clippy::exhaustive_structs,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::let_underscore_must_use,
        clippy::let_underscore_untyped,
        clippy::missing_assert_message,
        clippy::missing_panics_doc,
        clippy::must_use_candidate,
        clippy::panic,
        clippy::print_stdout,
        clippy::too_many_lines,
        clippy::unwrap_in_result,
        clippy::unwrap_used,
        reason = "Not useful in unit tests"
    )
)]

pub mod address;
pub mod entities;
pub mod interface;

/// Shared test functionality.
#[cfg(test)]
pub mod tests {
    pub mod common;
}
