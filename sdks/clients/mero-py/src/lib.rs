//! Python bindings for the Calimero 1.0 client, using PyO3.
//!
//! This crate provides Python bindings that can be built independently
//! without requiring the full Calimero workspace.
//!
//! ## Module Structure
//!
//! The extension is `calimero_client_py._native`, under the Python package
//! (`calimero_client_py`), whose routes are a client generated from
//! core's OpenAPI document. What a generated client cannot be made of is here:
//!
//! - [`client`] - `SealedClient`: the attestation and the sealed channel the
//!   generated client's httpx transport sends through, the recovery between
//!   nodes, the event streams, and the keys the client signs with
//! - [`passkey`] - `SoftPasskey`, a software passkey for tests and scripts
//! - [`error`] - `CloudError`
//! - [`convert`] - Python <-> JSON

pub mod client;
pub mod convert;
pub mod error;
pub mod passkey;

use pyo3::prelude::*;

/// The native half of `calimero_client_py`.
#[pymodule]
fn _native(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<client::PyCloudClient>()?;
    m.add_class::<passkey::PySoftPasskey>()?;
    m.add("CloudError", m.py().get_type::<error::CloudError>())?;
    m.add("LoginRequired", m.py().get_type::<error::LoginRequired>())?;
    m.add("WrongNode", m.py().get_type::<error::WrongNode>())?;
    m.add(
        "FeatureNotActive",
        m.py().get_type::<error::FeatureNotActive>(),
    )?;

    m.add("VERSION", env!("CARGO_PKG_VERSION"))?;

    Ok(())
}
