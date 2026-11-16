//! `SoftPasskey`: a software passkey for tests and scripts.

use std::sync::Arc;

use calimero_client::Passkey;
use calimero_identity::testing::SoftAuthenticator;
use pyo3::prelude::*;
use tokio::sync::Mutex;

use crate::convert::{to_json, to_py};

/// clusters. Real users sign in from a browser; services use API keys.
#[pyclass(name = "SoftPasskey")]
pub struct PySoftPasskey {
    pub(crate) inner: Arc<Mutex<SoftAuthenticator>>,
}

#[pymethods]
impl PySoftPasskey {
    #[new]
    #[pyo3(signature = (rp_id = "localhost", origin = "http://localhost:3000"))]
    fn new(rp_id: &str, origin: &str) -> Self {
        Self {
            inner: Arc::new(Mutex::new(SoftAuthenticator::new(rp_id, origin))),
        }
    }

    #[getter]
    fn credential_id(&self) -> String {
        self.inner.blocking_lock().credential_id()
    }

    /// Answer `navigator.credentials.create(options)`: a new credential for
    /// the `options` a ceremony's `begin` answered. Returns the registration
    /// (`{attestationObject, clientDataJson}`, base64url) its `finish` takes.
    fn create(&self, py: Python<'_>, options: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let options = to_json(py, Some(options))?;
        let registration = self.inner.blocking_lock().create(&options);
        to_py(py, &registration)
    }

    /// Answer `navigator.credentials.get`, signing `challenge` (the bytes a
    /// `412 CONFIRM_REQUIRED` or a sign-in's `begin` named). Returns the
    /// assertion (`{credentialId, authenticatorData, clientDataJson,
    /// signature}`, base64url).
    fn get(&self, py: Python<'_>, challenge: &[u8]) -> PyResult<PyObject> {
        let assertion = self.inner.blocking_lock().get(challenge);
        to_py(py, &assertion)
    }
}
