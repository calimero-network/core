//! Every test in the crate, one file per module under test.
//!
//! They live here rather than inside the modules they cover so that reading
//! `payload.rs` is reading the wire format and nothing else. Each file reaches
//! into its subject through `crate::` paths.
//!
//! `wire.rs` is the exception to the one-file-per-module rule, and deliberately
//! so: decode-side hostility (a forward tag, a truncated buffer, trailing
//! bytes) is a property of the borsh encoding rather than of any one type, and
//! splitting those cases across `payload.rs` and `op.rs` would let a whole
//! class of malformed input go untested in one of them without it showing.

mod support;

mod authorship;
mod op;
mod payload;
mod scope;
mod wire;
