//! The `.mpk` reader lives in `calimero-bundle` (`archive`); this node checks a
//! manifest's `minRuntimeVersion` against its own release.

use semver::Version;

pub use calimero_bundle::archive::*;

/// This node's release, which a bundle's `minRuntimeVersion` must not exceed.
pub fn runtime_version() -> Version {
    Version::parse(env!("CALIMERO_RELEASE_VERSION"))
        .expect("CALIMERO_RELEASE_VERSION is set by build.rs from the crate version")
}
