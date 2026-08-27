//! One route from a named `bytecode_id` to an installed application, from the
//! single source this node is configured with. See AGENTS.md for the contract.

pub mod http;
pub mod port;
pub mod registry;
pub mod source;

mod downloader;

pub use downloader::{ApplicationDownloader, Outcome};
pub use port::{ApplicationStore, InstalledApplication};
pub use registry::{RegistryConfig, RegistryCoords, RegistryCoordsBuf, RegistryMode};
pub use source::{app_source, AppRequest, AppSource};
