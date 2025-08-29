//! Python bindings for Calimero client using PyO3

use std::str::FromStr;
use std::sync::Arc;

use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use hex;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use tokio::runtime::Runtime;
use url::Url;

use crate::client::Client;
use crate::connection::ConnectionInfo;
use crate::traits::ClientStorage;
use crate::{CliAuthenticator, ClientError, JwtToken};

/// Python module for Calimero client bindings
#[pymodule]
fn calimero_client_py_bindings(_py: Python<'_>, m: &PyModule) -> PyResult<()> {
    // Register classes
    m.add_class::<PyConnectionInfo>()?;
    m.add_class::<PyClient>()?;
    m.add_class::<PyJwtToken>()?;
    m.add_class::<PyClientError>()?;
    m.add_class::<PyAuthMode>()?;

    // Register functions
    m.add_function(wrap_pyfunction!(create_connection, m)?)?;
    m.add_function(wrap_pyfunction!(create_client, m)?)?;

    // Add constants
    m.add("VERSION", env!("CARGO_PKG_VERSION"))?;

    Ok(())
}

/// Python wrapper for ClientError
#[pyclass(name = "ClientError")]
#[derive(Debug)]
pub struct PyClientError {
    error_type: String,
    message: String,
}

#[pymethods]
impl PyClientError {
    #[new]
    fn new(error_type: &str, message: &str) -> Self {
        Self {
            error_type: error_type.to_string(),
            message: message.to_string(),
        }
    }

    #[getter]
    fn error_type(&self) -> &str {
        &self.error_type
    }

    #[getter]
    fn message(&self) -> &str {
        &self.message
    }

    fn __str__(&self) -> String {
        format!("{}: {}", self.error_type, self.message)
    }

    fn __repr__(&self) -> String {
        format!(
            "ClientError(error_type='{}', message='{}')",
            self.error_type, self.message
        )
    }
}

impl From<ClientError> for PyClientError {
    fn from(err: ClientError) -> Self {
        match err {
            ClientError::Network { message } => Self {
                error_type: "Network".to_string(),
                message,
            },
            ClientError::Authentication { message } => Self {
                error_type: "Authentication".to_string(),
                message,
            },
            ClientError::Storage { message } => Self {
                error_type: "Storage".to_string(),
                message,
            },
            ClientError::Internal { message } => Self {
                error_type: "Internal".to_string(),
                message,
            },
        }
    }
}

/// Python wrapper for AuthMode
#[pyclass(name = "AuthMode")]
#[derive(Debug, Clone, Copy)]
pub struct PyAuthMode {
    mode: crate::connection::AuthMode,
}

#[pymethods]
impl PyAuthMode {
    #[new]
    fn new(mode: &str) -> PyResult<Self> {
        let mode = match mode.to_lowercase().as_str() {
            "required" => crate::connection::AuthMode::Required,
            "none" => crate::connection::AuthMode::None,
            _ => {
                return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                    "AuthMode must be 'required' or 'none'",
                ))
            }
        };
        Ok(Self { mode })
    }

    #[getter]
    fn value(&self) -> &str {
        match self.mode {
            crate::connection::AuthMode::Required => "required",
            crate::connection::AuthMode::None => "none",
        }
    }

    fn __str__(&self) -> &str {
        self.value()
    }

    fn __repr__(&self) -> String {
        format!("AuthMode('{}')", self.value())
    }
}

/// Python wrapper for JwtToken
#[pyclass(name = "JwtToken")]
#[derive(Debug, Clone)]
pub struct PyJwtToken {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: Option<i64>,
}

#[pymethods]
impl PyJwtToken {
    #[new]
    fn new(access_token: &str, refresh_token: Option<&str>, expires_at: Option<i64>) -> Self {
        Self {
            access_token: access_token.to_string(),
            refresh_token: refresh_token.map(|s| s.to_string()),
            expires_at,
        }
    }

    #[getter]
    fn access_token(&self) -> &str {
        &self.access_token
    }

    #[getter]
    fn refresh_token(&self) -> Option<&str> {
        self.refresh_token.as_deref()
    }

    #[getter]
    fn expires_at(&self) -> Option<i64> {
        self.expires_at
    }

    fn is_expired(&self) -> bool {
        if let Some(expires_at) = self.expires_at {
            let now = chrono::Utc::now().timestamp();
            now >= expires_at
        } else {
            false
        }
    }

    fn __str__(&self) -> String {
        format!(
            "JwtToken(access_token='{}...', expires_at={:?})",
            &self.access_token[..self.access_token.len().min(10)],
            self.expires_at
        )
    }
}

impl From<JwtToken> for PyJwtToken {
    fn from(token: JwtToken) -> Self {
        Self {
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            expires_at: token.expires_at,
        }
    }
}

/// Python wrapper for ConnectionInfo
#[pyclass(name = "ConnectionInfo")]
pub struct PyConnectionInfo {
    inner: Arc<ConnectionInfo<CliAuthenticator, PyFileStorage>>,
    runtime: Arc<Runtime>,
}

#[pymethods]
impl PyConnectionInfo {
    #[new]
    fn new(api_url: &str, node_name: Option<&str>) -> PyResult<Self> {
        let runtime = Arc::new(
            Runtime::new()
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?,
        );

        let url = Url::parse(api_url).map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!("Invalid URL: {}", e))
        })?;

        let authenticator = CliAuthenticator::new();
        let storage = PyFileStorage::new();

        let connection = ConnectionInfo::new(
            url,
            node_name.map(|s| s.to_string()),
            authenticator,
            storage,
        );

        Ok(Self {
            inner: Arc::new(connection),
            runtime,
        })
    }

    #[getter]
    fn api_url(&self) -> String {
        self.inner.api_url.to_string()
    }

    #[getter]
    fn node_name(&self) -> Option<String> {
        self.inner.node_name.clone()
    }

    /// Make a GET request
    fn get(&self, path: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let path = path.to_string();

        Python::with_gil(|py| {
            let result = self
                .runtime
                .block_on(async move { inner.get::<serde_json::Value>(&path).await });

            match result {
                Ok(data) => Ok(json_to_python(py, &data)),
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Check if authentication is required
    fn detect_auth_mode(&self) -> PyResult<PyAuthMode> {
        let inner = self.inner.clone();

        let result = self
            .runtime
            .block_on(async move { inner.detect_auth_mode().await });

        match result {
            Ok(mode) => Ok(PyAuthMode { mode }),
            Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                "Client error: {}",
                e
            ))),
        }
    }
}

/// Python wrapper for Client
#[pyclass(name = "Client")]
pub struct PyClient {
    inner: Arc<Client<CliAuthenticator, PyFileStorage>>,
    runtime: Arc<Runtime>,
}

#[pymethods]
impl PyClient {
    #[new]
    fn new(connection: &PyConnectionInfo) -> PyResult<Self> {
        let runtime = Arc::new(
            Runtime::new()
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?,
        );

        // Extract the inner connection from the Arc
        let connection_inner = connection.inner.as_ref().clone();
        let client = Client::new(connection_inner).map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                "Failed to create client: {}",
                e
            ))
        })?;

        Ok(Self {
            inner: Arc::new(client),
            runtime,
        })
    }

    /// Get API URL
    fn get_api_url(&self) -> String {
        self.inner.api_url().to_string()
    }

    /// Get application information
    fn get_application(&self, app_id: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let app_id = app_id.parse::<ApplicationId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "Invalid application ID '{}': {}",
                app_id, e
            ))
        })?;

        Python::with_gil(|py| {
            let result = self
                .runtime
                .block_on(async move { inner.get_application(&app_id).await });

            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// List applications
    fn list_applications(&self) -> PyResult<PyObject> {
        let inner = self.inner.clone();

        Python::with_gil(|py| {
            let result = self
                .runtime
                .block_on(async move { inner.list_applications().await });

            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Get context
    fn get_context(&self, context_id: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "Invalid context ID '{}': {}",
                context_id, e
            ))
        })?;

        Python::with_gil(|py| {
            let result = self
                .runtime
                .block_on(async move { inner.get_context(&context_id).await });

            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// List contexts
    fn list_contexts(&self) -> PyResult<PyObject> {
        let inner = self.inner.clone();

        Python::with_gil(|py| {
            let result = self
                .runtime
                .block_on(async move { inner.list_contexts().await });

            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Install application from URL
    fn install_application(
        &self,
        url: &str,
        hash: Option<&str>,
        metadata: Option<&[u8]>,
    ) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let url = url.to_string();
        let hash = hash.map(|h| h.to_string());
        let metadata = metadata.unwrap_or(b"{}").to_vec();

        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                let url = url::Url::parse(&url).map_err(|e| eyre::eyre!("Invalid URL: {}", e))?;

                let hash = if let Some(hash_str) = hash {
                    let hash_bytes =
                        hex::decode(hash_str).map_err(|e| eyre::eyre!("Invalid hash: {}", e))?;
                    let hash_array: [u8; 32] = hash_bytes
                        .try_into()
                        .map_err(|_| eyre::eyre!("Hash must be 32 bytes"))?;
                    Some(calimero_primitives::hash::Hash::from(hash_array))
                } else {
                    None
                };

                let request = calimero_server_primitives::admin::InstallApplicationRequest::new(
                    url, hash, metadata,
                );

                inner.install_application(request).await
            });

            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Install development application from local path
    fn install_dev_application(&self, path: &str, metadata: Option<&[u8]>) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let path = path.to_string();
        let metadata = metadata.unwrap_or(b"{}").to_vec();

        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                let path = camino::Utf8PathBuf::from(path);
                let metadata = metadata;

                let request = calimero_server_primitives::admin::InstallDevApplicationRequest::new(
                    path, metadata,
                );

                inner.install_dev_application(request).await
            });

            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Uninstall application
    fn uninstall_application(&self, app_id: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let app_id = app_id.parse::<ApplicationId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "Invalid application ID '{}': {}",
                app_id, e
            ))
        })?;

        Python::with_gil(|py| {
            let result = self
                .runtime
                .block_on(async move { inner.uninstall_application(&app_id).await });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// List blobs
    fn list_blobs(&self) -> PyResult<PyObject> {
        let inner = self.inner.clone();

        Python::with_gil(|py| {
            let result = self
                .runtime
                .block_on(async move { inner.list_blobs().await });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Get blob info
    fn get_blob_info(&self, blob_id: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let blob_id = blob_id
            .parse::<calimero_primitives::blobs::BlobId>()
            .map_err(|e| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                    "Invalid blob ID '{}': {}",
                    blob_id, e
                ))
            })?;

        Python::with_gil(|py| {
            let result = self
                .runtime
                .block_on(async move { inner.get_blob_info(&blob_id).await });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Delete blob
    fn delete_blob(&self, blob_id: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let blob_id = blob_id
            .parse::<calimero_primitives::blobs::BlobId>()
            .map_err(|e| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                    "Invalid blob ID '{}': {}",
                    blob_id, e
                ))
            })?;

        Python::with_gil(|py| {
            let result = self
                .runtime
                .block_on(async move { inner.delete_blob(&blob_id).await });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Generate context identity
    fn generate_context_identity(&self) -> PyResult<PyObject> {
        let inner = self.inner.clone();

        Python::with_gil(|py| {
            let result = self
                .runtime
                .block_on(async move { inner.generate_context_identity().await });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Get peers count
    fn get_peers_count(&self) -> PyResult<PyObject> {
        let inner = self.inner.clone();

        Python::with_gil(|py| {
            let result = self
                .runtime
                .block_on(async move { inner.get_peers_count().await });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Create context
    fn create_context(
        &self,
        application_id: &str,
        protocol: &str,
        params: Option<&str>,
    ) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let application_id = application_id.parse::<ApplicationId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "Invalid application ID '{}': {}",
                application_id, e
            ))
        })?;

        let params = params.map(|p| p.as_bytes().to_vec()).unwrap_or_default();

        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                let request = calimero_server_primitives::admin::CreateContextRequest::new(
                    protocol.to_string(),
                    application_id,
                    None, // context_seed
                    params,
                );
                inner.create_context(request).await
            });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Delete context
    fn delete_context(&self, context_id: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "Invalid context ID '{}': {}",
                context_id, e
            ))
        })?;

        Python::with_gil(|py| {
            let result = self
                .runtime
                .block_on(async move { inner.delete_context(&context_id).await });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Get context storage
    fn get_context_storage(&self, context_id: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "Invalid context ID '{}': {}",
                context_id, e
            ))
        })?;

        Python::with_gil(|py| {
            let result = self
                .runtime
                .block_on(async move { inner.get_context_storage(&context_id).await });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Get context identities
    fn get_context_identities(&self, context_id: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "Invalid context ID '{}': {}",
                context_id, e
            ))
        })?;

        Python::with_gil(|py| {
            let result = self
                .runtime
                .block_on(async move { inner.get_context_identities(&context_id, false).await });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Get context client keys
    fn get_context_client_keys(&self, context_id: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "Invalid context ID '{}': {}",
                context_id, e
            ))
        })?;

        Python::with_gil(|py| {
            let result = self
                .runtime
                .block_on(async move { inner.get_context_client_keys(&context_id).await });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Sync context
    fn sync_context(&self, context_id: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "Invalid context ID '{}': {}",
                context_id, e
            ))
        })?;

        Python::with_gil(|py| {
            let result = self
                .runtime
                .block_on(async move { inner.sync_context(&context_id).await });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Invite to context
    fn invite_to_context(
        &self,
        context_id: &str,
        inviter_id: &str,
        invitee_id: &str,
    ) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "Invalid context ID '{}': {}",
                context_id, e
            ))
        })?;
        let inviter_id = inviter_id
            .parse::<calimero_primitives::identity::PublicKey>()
            .map_err(|e| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                    "Invalid inviter ID '{}': {}",
                    inviter_id, e
                ))
            })?;
        let invitee_id = invitee_id
            .parse::<calimero_primitives::identity::PublicKey>()
            .map_err(|e| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                    "Invalid invitee ID '{}': {}",
                    invitee_id, e
                ))
            })?;

        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                let request = calimero_server_primitives::admin::InviteToContextRequest::new(
                    context_id, inviter_id, invitee_id,
                );
                inner.invite_to_context(request).await
            });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Join context
    fn join_context(
        &self,
        context_id: &str,
        invitee_id: &str,
        invitation_payload: &str,
    ) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "Invalid context ID '{}': {}",
                context_id, e
            ))
        })?;
        let invitee_id = invitee_id
            .parse::<calimero_primitives::identity::PublicKey>()
            .map_err(|e| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                    "Invalid invitee ID '{}': {}",
                    invitee_id, e
                ))
            })?;

        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                // For now, let's just try to join using the context_id and invitee_id
                // The invitation payload contains the necessary protocol/network/contract info
                // but we'll use the existing context for now
                let request = calimero_server_primitives::admin::JoinContextRequest::new(
                    calimero_primitives::context::ContextInvitationPayload::try_from(
                        invitation_payload,
                    )
                    .map_err(|e| eyre::eyre!("Failed to parse invitation payload: {}", e))?,
                );
                inner.join_context(request).await
            });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Execute function call via JSON-RPC
    fn execute_function(
        &self,
        context_id: &str,
        method: &str,
        args: &str,
        executor_public_key: &str,
    ) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "Invalid context ID '{}': {}",
                context_id, e
            ))
        })?;
        let executor_public_key = executor_public_key
            .parse::<calimero_primitives::identity::PublicKey>()
            .map_err(|e| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                    "Invalid executor public key '{}': {}",
                    executor_public_key, e
                ))
            })?;

        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                // Parse args as JSON
                let args_value: serde_json::Value = serde_json::from_str(args)
                    .map_err(|e| eyre::eyre!("Invalid JSON args: {}", e))?;

                let execution_request = calimero_server_primitives::jsonrpc::ExecutionRequest::new(
                    context_id,
                    method.to_string(),
                    args_value,
                    executor_public_key,
                    vec![], // substitute aliases
                );

                let request = calimero_server_primitives::jsonrpc::Request::new(
                    calimero_server_primitives::jsonrpc::Version::TwoPointZero,
                    calimero_server_primitives::jsonrpc::RequestId::String("1".to_string()),
                    calimero_server_primitives::jsonrpc::RequestPayload::Execute(execution_request),
                );
                inner.execute_jsonrpc(request).await
            });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Grant permissions to users in a context
    fn grant_permissions(&self, context_id: &str, permissions: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "Invalid context ID '{}': {}",
                context_id, e
            ))
        })?;

        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                // Parse permissions as JSON array of [public_key, capability] pairs
                let permissions_value: Vec<(
                    calimero_primitives::identity::PublicKey,
                    calimero_context_config::types::Capability,
                )> = serde_json::from_str(permissions)
                    .map_err(|e| eyre::eyre!("Invalid JSON permissions: {}", e))?;

                inner
                    .grant_permissions(&context_id, permissions_value)
                    .await
            });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Revoke permissions from users in a context
    fn revoke_permissions(&self, context_id: &str, permissions: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "Invalid context ID '{}': {}",
                context_id, e
            ))
        })?;

        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                // Parse permissions as JSON array of [public_key, capability] pairs
                let permissions_value: Vec<(
                    calimero_primitives::identity::PublicKey,
                    calimero_context_config::types::Capability,
                )> = serde_json::from_str(permissions)
                    .map_err(|e| eyre::eyre!("Invalid JSON permissions: {}", e))?;

                inner
                    .revoke_permissions(&context_id, permissions_value)
                    .await
            });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Update context application
    fn update_context_application(
        &self,
        context_id: &str,
        application_id: &str,
        executor_public_key: &str,
    ) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "Invalid context ID '{}': {}",
                context_id, e
            ))
        })?;
        let application_id = application_id.parse::<ApplicationId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "Invalid application ID '{}': {}",
                application_id, e
            ))
        })?;
        let executor_public_key = executor_public_key
            .parse::<calimero_primitives::identity::PublicKey>()
            .map_err(|e| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                    "Invalid executor public key '{}': {}",
                    executor_public_key, e
                ))
            })?;

        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                let request =
                    calimero_server_primitives::admin::UpdateContextApplicationRequest::new(
                        application_id,
                        executor_public_key,
                    );
                inner.update_context_application(&context_id, request).await
            });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Get proposal information
    fn get_proposal(&self, context_id: &str, proposal_id: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "Invalid context ID '{}': {}",
                context_id, e
            ))
        })?;
        let proposal_id = proposal_id
            .parse::<calimero_primitives::hash::Hash>()
            .map_err(|e| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                    "Invalid proposal ID '{}': {}",
                    proposal_id, e
                ))
            })?;

        Python::with_gil(|py| {
            let result = self
                .runtime
                .block_on(async move { inner.get_proposal(&context_id, &proposal_id).await });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Get proposal approvers
    fn get_proposal_approvers(&self, context_id: &str, proposal_id: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "Invalid context ID '{}': {}",
                context_id, e
            ))
        })?;
        let proposal_id = proposal_id
            .parse::<calimero_primitives::hash::Hash>()
            .map_err(|e| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                    "Invalid proposal ID '{}': {}",
                    proposal_id, e
                ))
            })?;

        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                inner
                    .get_proposal_approvers(&context_id, &proposal_id)
                    .await
            });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// List proposals in a context
    fn list_proposals(&self, context_id: &str, args: Option<&str>) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "Invalid context ID '{}': {}",
                context_id, e
            ))
        })?;

        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                let args_value = if let Some(args_str) = args {
                    serde_json::from_str(args_str)
                        .map_err(|e| eyre::eyre!("Invalid JSON args: {}", e))?
                } else {
                    serde_json::Value::Null
                };

                inner.list_proposals(&context_id, args_value).await
            });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Sync all contexts
    fn sync_all_contexts(&self) -> PyResult<PyObject> {
        let inner = self.inner.clone();

        Python::with_gil(|py| {
            let result = self
                .runtime
                .block_on(async move { inner.sync_all_contexts().await });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Create context identity alias
    fn create_context_identity_alias(
        &self,
        context_id: &str,
        alias: &str,
        public_key: &str,
    ) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "Invalid context ID '{}': {}",
                context_id, e
            ))
        })?;
        let public_key = public_key
            .parse::<calimero_primitives::identity::PublicKey>()
            .map_err(|e| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                    "Invalid public key '{}': {}",
                    public_key, e
                ))
            })?;

        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                let alias_obj = calimero_primitives::alias::Alias::<
                    calimero_primitives::identity::PublicKey,
                >::from_str(alias)
                .map_err(|e| eyre::eyre!("Invalid alias: {}", e))?;
                let request = calimero_server_primitives::admin::CreateAliasRequest {
                    alias: alias_obj,
                    value: calimero_server_primitives::admin::CreateContextIdentityAlias {
                        identity: public_key,
                    },
                };
                inner
                    .create_context_identity_alias(&context_id, request)
                    .await
            });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Create context alias
    fn create_context_alias(&self, alias: &str, context_id: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "Invalid context ID '{}': {}",
                context_id, e
            ))
        })?;

        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                let alias_obj = calimero_primitives::alias::Alias::<ContextId>::from_str(alias)
                    .map_err(|e| eyre::eyre!("Invalid alias: {}", e))?;

                inner.create_alias(alias_obj, context_id, None).await
            });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Create application alias
    fn create_application_alias(&self, alias: &str, application_id: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let application_id = application_id.parse::<ApplicationId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "Invalid application ID '{}': {}",
                application_id, e
            ))
        })?;

        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                let alias_obj = calimero_primitives::alias::Alias::<ApplicationId>::from_str(alias)
                    .map_err(|e| eyre::eyre!("Invalid alias: {}", e))?;

                inner.create_alias(alias_obj, application_id, None).await
            });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Delete context alias
    fn delete_context_alias(&self, alias: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();

        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                let alias_obj = calimero_primitives::alias::Alias::<ContextId>::from_str(alias)
                    .map_err(|e| eyre::eyre!("Invalid alias: {}", e))?;

                inner.delete_alias(alias_obj, None).await
            });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Delete context identity alias
    fn delete_context_identity_alias(&self, alias: &str, context_id: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "Invalid context ID '{}': {}",
                context_id, e
            ))
        })?;

        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                let alias_obj = calimero_primitives::alias::Alias::<
                    calimero_primitives::identity::PublicKey,
                >::from_str(alias)
                .map_err(|e| eyre::eyre!("Invalid alias: {}", e))?;

                inner.delete_alias(alias_obj, Some(context_id)).await
            });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Delete application alias
    fn delete_application_alias(&self, alias: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();

        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                let alias_obj = calimero_primitives::alias::Alias::<ApplicationId>::from_str(alias)
                    .map_err(|e| eyre::eyre!("Invalid alias: {}", e))?;

                inner.delete_alias(alias_obj, None).await
            });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// List context aliases
    fn list_context_aliases(&self) -> PyResult<PyObject> {
        let inner = self.inner.clone();

        Python::with_gil(|py| {
            let result = self
                .runtime
                .block_on(async move { inner.list_aliases::<ContextId>(None).await });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// List context identity aliases
    fn list_context_identity_aliases(&self, context_id: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "Invalid context ID '{}': {}",
                context_id, e
            ))
        })?;

        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                inner
                    .list_aliases::<calimero_primitives::identity::PublicKey>(Some(context_id))
                    .await
            });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// List application aliases
    fn list_application_aliases(&self) -> PyResult<PyObject> {
        let inner = self.inner.clone();

        Python::with_gil(|py| {
            let result = self
                .runtime
                .block_on(async move { inner.list_aliases::<ApplicationId>(None).await });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Lookup context alias
    fn lookup_context_alias(&self, alias: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();

        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                let alias_obj = calimero_primitives::alias::Alias::<ContextId>::from_str(alias)
                    .map_err(|e| eyre::eyre!("Invalid alias: {}", e))?;

                inner.lookup_alias(alias_obj, None).await
            });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Lookup context identity alias
    fn lookup_context_identity_alias(&self, alias: &str, context_id: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "Invalid context ID '{}': {}",
                context_id, e
            ))
        })?;

        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                let alias_obj = calimero_primitives::alias::Alias::<
                    calimero_primitives::identity::PublicKey,
                >::from_str(alias)
                .map_err(|e| eyre::eyre!("Invalid alias: {}", e))?;

                inner.lookup_alias(alias_obj, Some(context_id)).await
            });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Lookup application alias
    fn lookup_application_alias(&self, alias: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();

        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                let alias_obj = calimero_primitives::alias::Alias::<ApplicationId>::from_str(alias)
                    .map_err(|e| eyre::eyre!("Invalid alias: {}", e))?;

                inner.lookup_alias(alias_obj, None).await
            });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Resolve context alias
    fn resolve_context_alias(&self, alias: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();

        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                let alias_obj = calimero_primitives::alias::Alias::<ContextId>::from_str(alias)
                    .map_err(|e| eyre::eyre!("Invalid alias: {}", e))?;

                inner.resolve_alias(alias_obj, None).await
            });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Resolve context identity alias
    fn resolve_context_identity_alias(&self, alias: &str, context_id: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "Invalid context ID '{}': {}",
                context_id, e
            ))
        })?;

        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                let alias_obj = calimero_primitives::alias::Alias::<
                    calimero_primitives::identity::PublicKey,
                >::from_str(alias)
                .map_err(|e| eyre::eyre!("Invalid alias: {}", e))?;

                inner.resolve_alias(alias_obj, Some(context_id)).await
            });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Resolve application alias
    fn resolve_application_alias(&self, alias: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();

        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                let alias_obj = calimero_primitives::alias::Alias::<ApplicationId>::from_str(alias)
                    .map_err(|e| eyre::eyre!("Invalid alias: {}", e))?;

                inner.resolve_alias(alias_obj, None).await
            });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }

    /// Create alias generic (Python wrapper for backward compatibility)
    fn create_alias_generic(
        &self,
        alias: &str,
        value: &str,
        scope: Option<&str>,
    ) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let alias_str = alias.to_string();
        let value_str = value.to_string();
        let _scope_str = scope.map(|s| s.to_string());

        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                // This is a simplified wrapper - in practice, you'd need to know the type T
                // For now, we'll use ContextId as a default type
                let alias_obj =
                    calimero_primitives::alias::Alias::<ContextId>::from_str(&alias_str)
                        .map_err(|e| eyre::eyre!("Invalid alias: {}", e))?;

                // Parse the value as ContextId
                let value_obj = value_str
                    .parse::<ContextId>()
                    .map_err(|e| eyre::eyre!("Invalid value: {}", e))?;

                // Create the alias
                inner.create_alias(alias_obj, value_obj, None).await
            });

            match result {
                Ok(data) => {
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to serialize response: {}",
                            e
                        ))
                    })?;
                    Ok(json_to_python(py, &json_data))
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Client error: {}",
                    e
                ))),
            }
        })
    }
}

/// Simple file-based storage implementation for Python
#[derive(Clone)]
struct PyFileStorage;

impl PyFileStorage {
    fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl ClientStorage for PyFileStorage {
    async fn save_tokens(&self, _node_name: &str, _tokens: &JwtToken) -> eyre::Result<()> {
        // For Python bindings, we'll use a simple in-memory approach
        Ok(())
    }

    async fn load_tokens(&self, _node_name: &str) -> eyre::Result<Option<JwtToken>> {
        // Return None for now - tokens would need to be managed by Python code
        Ok(None)
    }
}

/// Create a new connection
#[pyfunction]
fn create_connection(api_url: &str, node_name: Option<&str>) -> PyResult<PyConnectionInfo> {
    PyConnectionInfo::new(api_url, node_name)
}

/// Create a new client
#[pyfunction]
fn create_client(connection: &PyConnectionInfo) -> PyResult<PyClient> {
    PyClient::new(connection)
}

/// Convert serde_json::Value to Python object
fn json_to_python(py: Python, value: &serde_json::Value) -> PyObject {
    match value {
        serde_json::Value::Null => py.None(),
        serde_json::Value::Bool(b) => b.into_py(py),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.into_py(py)
            } else if let Some(f) = n.as_f64() {
                f.into_py(py)
            } else {
                n.to_string().into_py(py)
            }
        }
        serde_json::Value::String(s) => s.into_py(py),
        serde_json::Value::Array(arr) => {
            let list = PyList::new(py, Vec::<PyObject>::new());
            for item in arr {
                list.append(json_to_python(py, item)).unwrap();
            }
            list.into_py(py)
        }
        serde_json::Value::Object(obj) => {
            let dict = PyDict::new(py);
            for (k, v) in obj {
                dict.set_item(k, json_to_python(py, v)).unwrap();
            }
            dict.into_py(py)
        }
    }
}
