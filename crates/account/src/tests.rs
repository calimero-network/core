//! Every test in the crate, one file per module under test.
//!
//! They live here rather than inside the modules they cover so that reading
//! `device.rs` is reading the device model and nothing else. Each file reaches
//! into its subject through `crate::` paths, which is also what keeps the tests
//! honest about visibility: anything a test needs and cannot see is a signal
//! about the API, not a reason to relax the module boundary.

mod support;

mod account;
mod device;
mod domain;
mod pairing;
mod revocation;
mod root_key;
mod signed;
mod wire;
