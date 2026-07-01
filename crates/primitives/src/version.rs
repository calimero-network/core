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

/// Maximum accepted length (in bytes) of any single [`Version`] string field
/// when decoding from borsh. Version metadata (semver, git describe, commit,
/// rustc) is short; this bound stops a peer from advertising a multi-gigabyte
/// length prefix that would force a huge allocation during handshake.
#[cfg(feature = "borsh")]
pub const MAX_VERSION_STRING_LEN: usize = 256;

/// Data structure for release version and build metadata (git describe, commit, rustc).
///
/// Used for protocol version exchange and for constructing version strings in
/// binaries from build-time env vars (e.g. `MEROD_VERSION`, `MEROD_BUILD`).
///
/// `BorshDeserialize` is hand-written (not derived) so each string field is
/// length-capped on decode — see [`MAX_VERSION_STRING_LEN`]. The derive would
/// trust an attacker-controlled length prefix.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "borsh", derive(borsh::BorshSerialize))]
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

#[cfg(feature = "borsh")]
const _: () = {
    use borsh::io::{Error, ErrorKind, Read};
    use borsh::BorshDeserialize;

    /// Read a borsh `String` (u32 length prefix + UTF-8 bytes) but reject a
    /// length above [`MAX_VERSION_STRING_LEN`] before allocating.
    fn read_capped_string<R: Read>(reader: &mut R) -> borsh::io::Result<String> {
        let len = u32::deserialize_reader(reader)? as usize;
        if len > MAX_VERSION_STRING_LEN {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "Version string exceeds maximum length",
            ));
        }
        let mut buf = vec![0u8; len];
        reader.read_exact(&mut buf)?;
        String::from_utf8(buf)
            .map_err(|_| Error::new(ErrorKind::InvalidData, "Version string is not valid UTF-8"))
    }

    impl BorshDeserialize for Version {
        fn deserialize_reader<R: Read>(reader: &mut R) -> borsh::io::Result<Self> {
            Ok(Self {
                version: read_capped_string(reader)?,
                build: read_capped_string(reader)?,
                commit: read_capped_string(reader)?,
                rustc_version: read_capped_string(reader)?,
            })
        }
    }
};

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

    #[cfg(feature = "borsh")]
    #[test]
    fn borsh_roundtrips() {
        let version = Version::from_build_env("1.2.3", "v1.2.3-5-gabc123", "abc123", "1.88.0");
        let bytes = borsh::to_vec(&version).expect("serialize");
        let decoded: Version = borsh::from_slice(&bytes).expect("deserialize");
        assert_eq!(decoded.version, version.version);
        assert_eq!(decoded.build, version.build);
        assert_eq!(decoded.commit, version.commit);
        assert_eq!(decoded.rustc_version, version.rustc_version);
    }

    #[cfg(feature = "borsh")]
    #[test]
    fn borsh_rejects_oversized_string_length_prefix() {
        use super::MAX_VERSION_STRING_LEN;

        // A crafted frame whose first field claims a huge length must be
        // rejected at the length check, before allocating — no multi-GB alloc,
        // no read of gigabytes that aren't there.
        let mut bytes = Vec::new();
        let huge = (MAX_VERSION_STRING_LEN as u32) + 1;
        bytes.extend_from_slice(&huge.to_le_bytes());
        // No payload bytes follow; the length check must fire first.

        let result: Result<Version, _> = borsh::from_slice(&bytes);
        assert!(result.is_err(), "oversized length prefix must be rejected");

        // A string exactly at the cap is accepted.
        let at_cap = "a".repeat(MAX_VERSION_STRING_LEN);
        let version = Version::from_build_env(&at_cap, "", "", "");
        let ok_bytes = borsh::to_vec(&version).expect("serialize");
        let decoded: Version = borsh::from_slice(&ok_bytes).expect("deserialize at cap");
        assert_eq!(decoded.version.len(), MAX_VERSION_STRING_LEN);
    }
}
