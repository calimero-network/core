//! One route from "a group named a `bytecode_id`" to an installed, executable
//! application, from the single source this node is configured with.
//!
//! The crate is a leaf on purpose. Its consumers are the node client and the
//! context handlers, so it reaches back into the node through
//! [`ApplicationStore`] rather than naming any of them.

pub mod http;
pub mod port;
pub mod registry;
pub mod source;

mod downloader;

pub use downloader::{ApplicationDownloader, DownloadError, Outcome};
pub use port::{ApplicationStore, InstalledApplication};
pub use registry::{RegistryConfig, RegistryCoords, RegistryMode};
pub use source::{app_source, AppRequest, AppSource};
