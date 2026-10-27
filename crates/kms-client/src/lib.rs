//! The mero-kms client a TEE node fetches its keys with: the KMS is verified
//! against the signed mero-tee release policy (or a pinned allowlist), the
//! request is bound to an attested quote, and every key comes back sealed to
//! the TD. Moved here from v1's `merod`, unchanged.
//!
//! - [`kms`]: attestation, key fetch, probe.
//! - [`policy`]: the signed release policy.
//! - [`config`]: the `[tee.kms]` settings.
//! - [`disk_key`]: the data-disk unlock key (`merod-v2 kms disk-key`).

pub mod config;
pub mod disk_key;
pub mod kms;
pub mod policy;

pub use kms::*;
