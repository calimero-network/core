//! Fetches a signed release (`.mpk`) by `package@version` from the operator's package
//! registry (`source::http::HttpRegistry`, behind the node's `--registry-url`). The bytes
//! come back unverified: the node checks the signature and digests before installing.
//!
//! Spec: [Installing a release](https://calimero-network.github.io/core/protocol/applications/#installing-a-release).
//! Crate guide: [`crates/app-downloader/AGENTS.md`](https://github.com/calimero-network/core/blob/master/crates/app-downloader/AGENTS.md).

pub mod port;
pub mod registry;
pub mod source;

mod downloader;

pub use downloader::{ApplicationDownloader, Outcome};
pub use port::{ApplicationStore, InstalledApplication};
pub use registry::{RegistryConfig, RegistryCoords, RegistryCoordsBuf, RegistryMode};
pub use source::{app_source, AppRequest, AppSource};
