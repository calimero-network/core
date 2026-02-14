//! Version information and build metadata.
//!
//! Mirrors the approach used in [nearcore](https://github.com/near/nearcore):
//! the `Version` struct lives in primitives; binaries set version env vars
//! in their own build scripts and construct `Version` from those.
//!
//! **Intended use:**
//! - **Protocol / network version exchange**: when sending or comparing version info
//!   between nodes or in APIs (serialization via Serde/Borsh).
//! - **Building version strings in binaries**: use [`Version::from_build_env`] with
//!   `env!("...")` to construct a value for display or logging.

use serde::{Deserialize, Serialize};

/// Data structure for release version and build metadata (git describe, commit, rustc).
///
/// Used for protocol version exchange and for constructing version strings in
/// binaries from build-time env vars (e.g. `MEROD_VERSION`, `MEROD_BUILD`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshSerialize, borsh::BorshDeserialize)
)]
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

impl Version {
    /// Build from build-time env vars (e.g. in binaries that set `MEROD_*` / `MEROCTL_*`).
    pub fn from_build_env(version: &str, build: &str, commit: &str, rustc_version: &str) -> Self {
        Self {
            version: version.to_string(),
            build: build.to_string(),
            commit: commit.to_string(),
            rustc_version: rustc_version.to_string(),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::Version;

    #[test]
    fn from_build_env_sets_all_fields() {
        let version = Version::from_build_env("1.2.3", "v1.2.3-5-gabc123", "abc123", "1.88.0");

        assert_eq!(version.version, "1.2.3");
        assert_eq!(version.build, "v1.2.3-5-gabc123");
        assert_eq!(version.commit, "abc123");
        assert_eq!(version.rustc_version, "1.88.0");
    }

    #[test]
    fn display_uses_expected_format() {
        let version = Version::from_build_env("1.2.3", "v1.2.3-5-gabc123", "abc123", "1.88.0");

        assert_eq!(
            version.to_string(),
            "(release 1.2.3) (build v1.2.3-5-gabc123) (commit abc123) (rustc 1.88.0)"
        );
    }
}
