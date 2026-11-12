//! An app's `calimero:abi` read the way clients use it.
//!
//! - [`abi`]: the ABI JSON parsed whole, free-form keys included (`params`, `returns`,
//!   `errors`, `events`), with [`ty`] parsing its type strings.
//! - [`openapi`]: the ABI as an OpenAPI 3.1 document, which the node serves at
//!   `GET /apps/{app}/openapi.json` and `mero-app-build codegen` writes.
//! - [`codegen`]: typed TypeScript, Python, Swift and Kotlin clients on top of mero-js,
//!   calimero-client-py, swift-sdk and kotlin-sdk.
//! - [`diff`]: what changed between two ABIs and whether callers notice, which
//!   `mero-app-build abi-diff` prints and `mero-sign sign --previous` checks before signing.
//! - [`load`]: the ABI of a `.wasm` component, an `.mpk` bundle, a `manifest.json` or a JSON
//!   file.

pub mod abi;
pub mod codegen;
pub mod diff;
pub mod load;
pub mod openapi;
pub mod ty;

pub use abi::AppAbi;
