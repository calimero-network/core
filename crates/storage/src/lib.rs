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

pub mod address;
pub mod entities;
pub mod interface;
