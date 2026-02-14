//! Version information and build metadata.
//!
//! Mirrors the approach used in [nearcore](https://github.com/near/nearcore):
//! the `Version` struct lives in primitives; binaries set version env vars
//! in their own build scripts and construct `Version` from those.

use serde::{Deserialize, Serialize};

/// Data structure for release version and build metadata (git describe, commit, rustc).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "borsh", derive(borsh::BorshSerialize, borsh::BorshDeserialize))]
pub struct Version {
    /// Release version (e.g. from Cargo.toml or git tag).
    pub version: String,
    /// Build identifier (e.g. git describe).
    pub build: String,
    /// Git commit (short).
    pub commit: String,
    /// Rustc version used to build.
    pub rustc_version: String,
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "(release {}) (build {}) (commit {}) (rustc {})",
            self.version, self.build, self.commit, self.rustc_version,
        )
    }
}
