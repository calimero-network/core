//! The mero-kms client a TEE node fetches its keys with: the KMS is verified
//! against the signed mero-tee release policy (and nothing else), the request
//! is bound to an attested quote, and every key comes back sealed to the TD.
//!
//! - [`kms`]: attestation, key fetch, probe.
//! - [`policy`]: the signed release policy.
//! - [`config`]: the `[tee.kms]` settings.
//! - [`disk_key`]: the data-disk unlock key (`merod kms disk-key`).
//!
//! Spec: [What a node trusts its KMS on](https://calimero-network.github.io/core/protocol/tee-attestation/#what-a-node-trusts-its-kms-on),
//! [The storage key and the cluster key](https://calimero-network.github.io/core/protocol/encryption/#at-rest-the-storage-key-k_r-and-the-cluster-key-k_c).
//! Crate guide: [`crates/kms-client/AGENTS.md`](https://github.com/calimero-network/core/blob/master/crates/kms-client/AGENTS.md).

pub mod config;
pub mod disk_key;
pub mod kms;
pub mod policy;

pub use kms::*;
