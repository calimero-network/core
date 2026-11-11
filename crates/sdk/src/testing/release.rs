//! A shipped release's component, read from its signed `.mpk` (feature `releases`), so an
//! upgrade test starts on the exact code that wrote the state it upgrades.

use std::sync::Arc;

use calimero_bundle::VerifiedBundle;
use eyre::{Result, WrapErr};
use semver::Version;

/// The `calimero:app` component of `service` (`None`: a single-service release) in the
/// signed release `mpk`, after the checks a node makes of the bundle when it installs it:
/// the publisher's signature and the artifact's digest. `minRuntimeVersion` is the node's
/// to judge against its own release, so it is not checked here.
///
/// # Errors
///
/// A bundle that is not a signed `.mpk`, fails a check, or has no such service.
pub fn release_component(mpk: &[u8], service: Option<&str>) -> Result<Vec<u8>> {
    // Any release the bundle names is met: the harness is not a node release.
    let any_runtime = Version::new(u64::MAX, 0, 0);
    let bundle = VerifiedBundle::open(Arc::from(mpk), &any_runtime).wrap_err("open the release")?;
    let wasm = bundle
        .wasm(service)
        .wrap_err_with(|| format!("the release's component ({service:?})"))?;
    Ok(wasm.to_vec())
}
