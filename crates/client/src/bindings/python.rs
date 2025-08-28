//! Python bindings for Calimero client using PyO3
//!
//! This module provides Python bindings for the main client functionality,
//! including connection management, authentication, and API operations.

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::runtime::Runtime;

use crate::client::Client;
use crate::connection::ConnectionInfo;
use crate::{
    ClientError, CliAuthenticator, JwtToken,
};
use crate::traits::ClientStorage;
use calimero_primitives::{
    alias::Alias,
    application::ApplicationId,
    blobs::BlobId,
    context::{ContextId, ContextInvitationPayload},
    hash::Hash,
    identity::PublicKey,
};
use calimero_server_primitives::{
    admin::{
        CreateContextRequest, InstallApplicationRequest, InstallDevApplicationRequest,
        InviteToContextRequest, JoinContextRequest, UpdateContextApplicationRequest,
    },
    jsonrpc::{Request, RequestPayload, ExecutionRequest, Version, RequestId},
};
use calimero_context_config::types::Capability;
use url::Url;

// Type conversion helpers
fn python_to_string(_py: Python, obj: &PyAny) -> PyResult<String> {
    if let Ok(s) = obj.extract::<String>() {
        Ok(s)
    } else if let Ok(s) = obj.extract::<&str>() {
        Ok(s.to_string())
    } else {
        Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
            "Expected string or bytes"
        ))
    }
}

fn python_to_u64(_py: Python, obj: &PyAny) -> PyResult<u64> {
    if let Ok(n) = obj.extract::<u64>() {
        Ok(n)
    } else if let Ok(n) = obj.extract::<i64>() {
        if n < 0 {
            return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "Negative number not allowed"
            ));
        }
        Ok(n as u64)
    } else {
        Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
            "Expected integer"
        ))
    }
}

fn python_to_bytes(_py: Python, obj: &PyAny) -> PyResult<Vec<u8>> {
    if let Ok(bytes) = obj.extract::<Vec<u8>>() {
        Ok(bytes)
    } else if let Ok(py_bytes) = obj.downcast::<PyBytes>() {
        Ok(py_bytes.as_bytes().to_vec())
    } else if let Ok(s) = obj.extract::<String>() {
        Ok(s.into_bytes())
    } else {
        Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
            "Expected bytes, string, or list of integers"
        ))
    }
}

fn python_to_hashmap(py: Python, obj: &PyAny) -> PyResult<HashMap<String, serde_json::Value>> {
    if obj.is_none() {
        return Ok(HashMap::new());
    }
    
    if let Ok(dict) = obj.downcast::<PyDict>() {
        let mut map = HashMap::new();
        for (key, value) in dict.iter() {
            let key_str = key.extract::<String>()?;
            let json_value = python_to_json(py, value)?;
            map.insert(key_str, json_value);
        }
        Ok(map)
    } else {
        Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
            "Expected dictionary or None"
        ))
    }
}

// Complex type conversion helpers
fn python_to_url(py: Python, obj: &PyAny) -> PyResult<Url> {
    let url_str = python_to_string(py, obj)?;
    if url_str.is_empty() {
        return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
            "URL cannot be empty"
        ));
    }
    
    url_str.parse::<Url>().map_err(|e| {
        PyErr::new::<pyo3::exceptions::PyValueError, _>(
            format!("Invalid URL '{}': {}", url_str, e)
        )
    })
}

fn python_to_hash(py: Python, obj: &PyAny) -> PyResult<Hash> {
    if obj.is_none() {
        return Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
            "Expected hash string, got None"
        ));
    }
    
    let hash_str = python_to_string(py, obj)?;
    hash_str.parse::<Hash>().map_err(|e| {
        PyErr::new::<pyo3::exceptions::PyValueError, _>(
            format!("Invalid hash '{}': {}", hash_str, e)
        )
    })
}

fn python_to_capabilities(py: Python, obj: &PyAny) -> PyResult<Vec<Capability>> {
    if obj.is_none() {
        return Ok(Vec::new());
    }
    
    if let Ok(list) = obj.downcast::<PyList>() {
        let mut capabilities = Vec::new();
        for item in list.iter() {
            let capability_str = python_to_string(py, item)?;
            // Parse capability string to enum variant
            let capability = match capability_str.as_str() {
                "manage_application" | "ManageApplication" => Capability::ManageApplication,
                "manage_members" | "ManageMembers" => Capability::ManageMembers,
                "proxy" | "Proxy" => Capability::Proxy,
                _ => Capability::ManageApplication, // Default to ManageApplication
            };
            capabilities.push(capability);
        }
        Ok(capabilities)
    } else {
        Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
            "Expected list of capabilities or None"
        ))
    }
}

fn python_to_permissions_request(py: Python, obj: &PyAny) -> PyResult<Vec<(PublicKey, Capability)>> {
    if let Ok(list) = obj.downcast::<PyList>() {
        let mut permissions = Vec::new();
        for item in list.iter() {
            if let Ok(tuple) = item.downcast::<PyList>() {
                if tuple.len() != 2 {
                    return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                        "Each permission must be a tuple of (public_key, capability)"
                    ));
                }
                
                let public_key_str = python_to_string(py, &tuple[0])?;
                let public_key = public_key_str.parse::<PublicKey>().map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyValueError, _>(
                        format!("Invalid public key '{}': {}", public_key_str, e)
                    )
                })?;
                
                let capability_str = python_to_string(py, &tuple[1])?;
                // Parse capability string to enum variant
                let capability = match capability_str.as_str() {
                    "manage_application" | "ManageApplication" => Capability::ManageApplication,
                    "manage_members" | "ManageMembers" => Capability::ManageMembers,
                    "proxy" | "Proxy" => Capability::Proxy,
                    _ => Capability::ManageApplication, // Default to ManageApplication
                };
                
                permissions.push((public_key, capability));
            } else {
                return Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                    "Each permission must be a list/tuple of [public_key, capability]"
                ));
            }
        }
        Ok(permissions)
    } else {
        Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
            "Expected list of permissions"
        ))
    }
}

fn python_to_install_application_request(py: Python, obj: &PyAny) -> PyResult<InstallApplicationRequest> {
    if let Ok(dict) = obj.downcast::<PyDict>() {
        let url = if let Some(url_obj) = dict.get_item("url") {
            python_to_url(py, url_obj)?
        } else {
            return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "Missing required field 'url'"
            ));
        };
        
        let hash = if let Some(hash_obj) = dict.get_item("hash") {
            if hash_obj.is_none() {
                None
            } else {
                Some(python_to_hash(py, hash_obj)?)
            }
        } else {
            None
        };
        
        let metadata = if let Some(metadata_obj) = dict.get_item("metadata") {
            python_to_bytes(py, metadata_obj)?
        } else {
            Vec::new()
        };
        
        Ok(InstallApplicationRequest::new(url, hash, metadata))
    } else {
        Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
            "Expected dictionary for InstallApplicationRequest"
        ))
    }
}

fn python_to_install_dev_application_request(py: Python, obj: &PyAny) -> PyResult<InstallDevApplicationRequest> {
    let dict = obj.downcast::<PyDict>()?;
    
    let path = if let Some(path_obj) = dict.get_item("path") {
        let path_str = python_to_string(py, path_obj)?;
        if path_str.is_empty() {
            return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "Path cannot be empty"
            ));
        }
        camino::Utf8PathBuf::from(path_str)
    } else {
        return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
            "Missing required field 'path'"
        ));
    };
    
    let metadata = if let Some(metadata_obj) = dict.get_item("metadata") {
        python_to_bytes(py, metadata_obj)?
    } else {
        Vec::new()
    };
    
    Ok(InstallDevApplicationRequest::new(path, metadata))
}

fn python_to_create_context_request(py: Python, obj: &PyAny) -> PyResult<CreateContextRequest> {
    if let Ok(dict) = obj.downcast::<PyDict>() {
        let protocol = if let Some(protocol_obj) = dict.get_item("protocol") {
            python_to_string(py, protocol_obj)?
        } else {
            return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "Missing required field 'protocol'"
            ));
        };
        
        let application_id = if let Some(app_id_obj) = dict.get_item("application_id") {
            let app_id_str = python_to_string(py, app_id_obj)?;
            app_id_str.parse::<ApplicationId>().map_err(|e| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(
                    format!("Invalid application ID '{}': {}", app_id_str, e)
                )
            })?
        } else {
            return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "Missing required field 'application_id'"
            ));
        };
        
        let context_seed = if let Some(seed_obj) = dict.get_item("context_seed") {
            if seed_obj.is_none() {
                None
            } else {
                Some(python_to_hash(py, seed_obj)?)
            }
        } else {
            None
        };
        
        let initialization_params = if let Some(params_obj) = dict.get_item("initialization_params") {
            python_to_bytes(py, params_obj)?
        } else {
            Vec::new()
        };
        
        Ok(CreateContextRequest::new(
            protocol,
            application_id,
            context_seed,
            initialization_params,
        ))
    } else {
        Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
            "Expected dictionary for CreateContextRequest"
        ))
    }
}

fn python_to_join_context_request(py: Python, obj: &PyAny) -> PyResult<JoinContextRequest> {
    let dict = obj.downcast::<PyDict>()?;
    
    let invitation_payload = if let Some(payload_obj) = dict.get_item("invitation_payload") {
        if let Ok(payload_str) = payload_obj.extract::<String>() {
            // Try to parse as base58 string first
            payload_str.parse::<ContextInvitationPayload>().map_err(|e| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(
                    format!("Invalid invitation_payload: {}", e)
                )
            })?
        } else if let Ok(payload_bytes) = payload_obj.extract::<Vec<u8>>() {
            // If it's bytes, try to create from base58 encoding
            let base58_str = bs58::encode(&payload_bytes).into_string();
            base58_str.parse::<ContextInvitationPayload>().map_err(|e| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(
                    format!("Invalid invitation_payload bytes: {}", e)
                )
            })?
        } else {
            return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "invitation_payload must be a string or bytes"
            ));
        }
    } else {
        return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
            "Missing required field 'invitation_payload'"
        ));
    };
    
    Ok(JoinContextRequest::new(invitation_payload))
}

fn python_to_invite_to_context_request(py: Python, obj: &PyAny) -> PyResult<InviteToContextRequest> {
    let dict = obj.downcast::<PyDict>()?;
    
    let context_id = if let Some(context_id_obj) = dict.get_item("context_id") {
        let context_id_str = python_to_string(py, context_id_obj)?;
        context_id_str.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("Invalid context_id: {}", e)
            )
        })?
    } else {
        return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
            "Missing required field 'context_id'"
        ));
    };
    
    let inviter_id = if let Some(inviter_id_obj) = dict.get_item("inviter_id") {
        let inviter_id_str = python_to_string(py, inviter_id_obj)?;
        inviter_id_str.parse::<PublicKey>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("Invalid inviter_id: {}", e)
            )
        })?
    } else {
        return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
            "Missing required field 'inviter_id'"
        ));
    };
    
    let invitee_id = if let Some(invitee_id_obj) = dict.get_item("invitee_id") {
        let invitee_id_str = python_to_string(py, invitee_id_obj)?;
        invitee_id_str.parse::<PublicKey>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("Invalid invitee_id: {}", e)
            )
        })?
    } else {
        return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
            "Missing required field 'invitee_id'"
        ));
    };
    
    Ok(InviteToContextRequest::new(context_id, inviter_id, invitee_id))
}

fn python_to_update_context_application_request(py: Python, obj: &PyAny) -> PyResult<UpdateContextApplicationRequest> {
    if let Ok(dict) = obj.downcast::<PyDict>() {
        let application_id = if let Some(app_id_obj) = dict.get_item("application_id") {
            let app_id_str = python_to_string(py, app_id_obj)?;
            app_id_str.parse::<ApplicationId>().map_err(|e| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(
                    format!("Invalid application ID '{}': {}", app_id_str, e)
                )
            })?
        } else {
            return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "Missing required field 'application_id'"
            ));
        };
        
        let executor_public_key = if let Some(executor_obj) = dict.get_item("executor_public_key") {
            let executor_str = python_to_string(py, executor_obj)?;
            executor_str.parse::<PublicKey>().map_err(|e| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(
                    format!("Invalid executor public key '{}': {}", executor_str, e)
                )
            })?
        } else {
            return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "Missing required field 'executor_public_key'"
            ));
        };
        
        Ok(UpdateContextApplicationRequest::new(application_id, executor_public_key))
    } else {
        Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
            "Expected dictionary for UpdateContextApplicationRequest"
        ))
    }
}

fn python_to_jsonrpc_request(py: Python, obj: &PyAny) -> PyResult<Request<RequestPayload>> {
    let dict = obj.downcast::<PyDict>()?;
    
    let id = if let Some(id_obj) = dict.get_item("id") {
        if let Ok(id_str) = id_obj.extract::<String>() {
            Some(RequestId::String(id_str))
        } else if let Ok(id_num) = id_obj.extract::<u64>() {
            Some(RequestId::Number(id_num))
        } else {
            None
        }
    } else {
        None
    };
    
    let method = if let Some(method_obj) = dict.get_item("method") {
        python_to_string(py, method_obj)?
    } else {
        return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
            "Missing required field 'method'"
        ));
    };
    
    let params = if let Some(params_obj) = dict.get_item("params") {
        if let Ok(params_dict) = params_obj.downcast::<PyDict>() {
            let mut map = HashMap::new();
            for (key, value) in params_dict.iter() {
                let key_str = key.extract::<String>()?;
                // Convert Python object to JSON value using a simple approach
                let value_json = if let Ok(s) = value.extract::<String>() {
                    serde_json::Value::String(s)
                } else if let Ok(n) = value.extract::<i64>() {
                    serde_json::Value::Number(serde_json::Number::from(n))
                } else if let Ok(n) = value.extract::<f64>() {
                    if let Some(num) = serde_json::Number::from_f64(n) {
                        serde_json::Value::Number(num)
                    } else {
                        serde_json::Value::Null
                    }
                } else if let Ok(b) = value.extract::<bool>() {
                    serde_json::Value::Bool(b)
                } else if value.is_none() {
                    serde_json::Value::Null
                } else {
                    // For complex objects, just convert to string representation
                    serde_json::Value::String(format!("{:?}", value))
                };
                map.insert(key_str, value_json);
            }
            serde_json::Value::Object(serde_json::Map::from_iter(map))
        } else {
            serde_json::Value::Null
        }
    } else {
        serde_json::Value::Null
    };
    
    // Create a zero ContextId and PublicKey for now
    let zero_context_id = ContextId::from([0u8; 32]);
    let zero_public_key = PublicKey::from([0u8; 32]);
    
    let execution_request = ExecutionRequest::new(
        zero_context_id,
        method,
        params,
        zero_public_key,
        Vec::new(), // No substitutes for now
    );
    
    Ok(Request::new(
        Version::TwoPointZero,
        id.unwrap_or(RequestId::String("1".to_string())),
        RequestPayload::Execute(execution_request)
    ))
}

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
        format!("ClientError(error_type='{}', message='{}')", self.error_type, self.message)
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
            _ => return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "AuthMode must be 'required' or 'none'"
            )),
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
        format!("JwtToken(access_token='{}...', expires_at={:?})", 
                &self.access_token[..self.access_token.len().min(10)], 
                self.expires_at)
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
        let runtime = Arc::new(Runtime::new().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string())
        })?);
        
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
            let result = self.runtime.block_on(async move {
                inner.get::<serde_json::Value>(&path).await
            });
            
            match result {
                Ok(data) => Ok(data.into_py(py)),
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// Make a POST request
    fn post(&self, path: &str, body: &PyAny) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let path = path.to_string();
        
        Python::with_gil(|py| {
            // Convert Python object to JSON value
            let body_value = python_to_json(py, body)?;
            
            let result = self.runtime.block_on(async move {
                inner.post::<serde_json::Value, serde_json::Value>(&path, body_value).await
            });
            
            match result {
                Ok(data) => Ok(data.into_py(py)),
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// Make a DELETE request
    fn delete(&self, path: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let path = path.to_string();
        
        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                inner.delete::<serde_json::Value>(&path).await
            });
            
            match result {
                Ok(data) => Ok(data.into_py(py)),
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// Check if authentication is required
    fn detect_auth_mode(&self) -> PyResult<PyAuthMode> {
        let inner = self.inner.clone();
        
        let result = self.runtime.block_on(async move {
            inner.detect_auth_mode().await
        });
        
        match result {
            Ok(mode) => Ok(PyAuthMode { mode }),
            Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                format!("Client error: {}", e)
            )),
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
        let runtime = Arc::new(Runtime::new().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string())
        })?);
        
        // Extract the inner connection from the Arc
        let connection_inner = connection.inner.as_ref().clone();
        let client = Client::new(connection_inner).map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!("Failed to create client: {}", e))
        })?;
        
        Ok(Self {
            inner: Arc::new(client),
            runtime,
        })
    }

    /// Resolve an alias (simplified version for Python)
    fn resolve_alias(&self, alias: &str, alias_type: &str, scope: Option<&str>) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let alias_str = alias.to_string();
        let alias_type = alias_type.to_string();
        
        Python::with_gil(|py| {
            let result: Result<serde_json::Value, eyre::Report> = self.runtime.block_on(async move {
                match alias_type.as_str() {
                    "context" => {
                        let alias_instance = match Alias::<ContextId>::from_str(&alias_str) {
                            Ok(alias) => alias,
                            Err(e) => return Err(eyre::eyre!("Invalid context alias: {}", e))
                        };
                        let data = inner.resolve_alias(alias_instance, None).await?;
                        serde_json::to_value(data).map_err(|e| eyre::eyre!(e.to_string()))
                    },
                    "identity" => {
                        let alias_instance = match Alias::<PublicKey>::from_str(&alias_str) {
                            Ok(alias) => alias,
                            Err(e) => return Err(eyre::eyre!("Invalid identity alias: {}", e))
                        };
                        let context_scope = if let Some(s) = scope {
                            match ContextId::from_str(s) {
                                Ok(id) => Some(id),
                                Err(e) => return Err(eyre::eyre!("Invalid context ID in scope: {}", e))
                            }
                        } else {
                            None
                        };
                        let data = inner.resolve_alias(alias_instance, context_scope).await?;
                        serde_json::to_value(data).map_err(|e| eyre::eyre!(e.to_string()))
                    },
                    "application" => {
                        let alias_instance = match Alias::<ApplicationId>::from_str(&alias_str) {
                            Ok(alias) => alias,
                            Err(e) => return Err(eyre::eyre!("Invalid application alias: {}", e))
                        };
                        let data = inner.resolve_alias(alias_instance, None).await?;
                        serde_json::to_value(data).map_err(|e| eyre::eyre!(e.to_string()))
                    },
                    _ => Err(eyre::eyre!("Unsupported alias type: {}", alias_type))
                }
            });
            
            match result {
                Ok(json_data) => Ok(json_data.into_py(py)),
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// Create an alias (simplified version for Python)
    fn create_alias(&self, alias: &str, alias_type: &str, value: &PyAny, scope: Option<&str>) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let alias_str = alias.to_string();
        let alias_type = alias_type.to_string();
        
        Python::with_gil(|py| {
            let result: Result<serde_json::Value, eyre::Report> = self.runtime.block_on(async move {
                match alias_type.as_str() {
                    "context" => {
                        let alias_instance = match Alias::<ContextId>::from_str(&alias_str) {
                            Ok(alias) => alias,
                            Err(e) => return Err(eyre::eyre!("Invalid context alias: {}", e))
                        };
                        let context_id = match ContextId::from_str(&python_to_string(py, value)?) {
                            Ok(id) => id,
                            Err(e) => return Err(eyre::eyre!("Invalid context ID value: {}", e))
                        };
                        let data = inner.create_alias(alias_instance, context_id, None).await?;
                        serde_json::to_value(data).map_err(|e| eyre::eyre!(e.to_string()))
                    },
                    "identity" => {
                        let alias_instance = match Alias::<PublicKey>::from_str(&alias_str) {
                            Ok(alias) => alias,
                            Err(e) => return Err(eyre::eyre!("Invalid identity alias: {}", e))
                        };
                        let public_key = match PublicKey::from_str(&python_to_string(py, value)?) {
                            Ok(key) => key,
                            Err(e) => return Err(eyre::eyre!("Invalid public key value: {}", e))
                        };
                        let context_scope = if let Some(s) = scope {
                            match ContextId::from_str(s) {
                                Ok(id) => Some(id),
                                Err(e) => return Err(eyre::eyre!("Invalid context ID in scope: {}", e))
                            }
                        } else {
                            None
                        };
                        let data = inner.create_alias(alias_instance, public_key, context_scope).await?;
                        serde_json::to_value(data).map_err(|e| eyre::eyre!(e.to_string()))
                    },
                    "application" => {
                        let alias_instance = match Alias::<ApplicationId>::from_str(&alias_str) {
                            Ok(alias) => alias,
                            Err(e) => return Err(eyre::eyre!("Invalid application alias: {}", e))
                        };
                        let app_id = match ApplicationId::from_str(&python_to_string(py, value)?) {
                            Ok(id) => id,
                            Err(e) => return Err(eyre::eyre!("Invalid application ID value: {}", e))
                        };
                        let data = inner.create_alias(alias_instance, app_id, None).await?;
                        serde_json::to_value(data).map_err(|e| eyre::eyre!(e.to_string()))
                    },
                    _ => Err(eyre::eyre!("Unsupported alias type: {}", alias_type))
                }
            });
            
            match result {
                Ok(json_data) => Ok(json_data.into_py(py)),
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// Delete an alias (simplified version for Python)
    fn delete_alias(&self, alias: &str, alias_type: &str, scope: Option<&str>) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let alias_str = alias.to_string();
        let alias_type = alias_type.to_string();
        
        Python::with_gil(|py| {
            let result: Result<serde_json::Value, eyre::Report> = self.runtime.block_on(async move {
                match alias_type.as_str() {
                    "context" => {
                        let alias_instance = match Alias::<ContextId>::from_str(&alias_str) {
                            Ok(alias) => alias,
                            Err(e) => return Err(eyre::eyre!("Invalid context alias: {}", e))
                        };
                        let data = inner.delete_alias(alias_instance, None).await?;
                        serde_json::to_value(data).map_err(|e| eyre::eyre!(e.to_string()))
                    },
                    "identity" => {
                        let alias_instance = match Alias::<PublicKey>::from_str(&alias_str) {
                            Ok(alias) => alias,
                            Err(e) => return Err(eyre::eyre!("Invalid identity alias: {}", e))
                        };
                        let context_scope = if let Some(s) = scope {
                            match ContextId::from_str(s) {
                                Ok(id) => Some(id),
                                Err(e) => return Err(eyre::eyre!("Invalid context ID in scope: {}", e))
                            }
                        } else {
                            None
                        };
                        let data = inner.delete_alias(alias_instance, context_scope).await?;
                        serde_json::to_value(data).map_err(|e| eyre::eyre!(e.to_string()))
                    },
                    "application" => {
                        let alias_instance = match Alias::<ApplicationId>::from_str(&alias_str) {
                            Ok(alias) => alias,
                            Err(e) => return Err(eyre::eyre!("Invalid application alias: {}", e))
                        };
                        let data = inner.delete_alias(alias_instance, None).await?;
                        serde_json::to_value(data).map_err(|e| eyre::eyre!(e.to_string()))
                    },
                    _ => Err(eyre::eyre!("Unsupported alias type: {}", alias_type))
                }
            });
            
            match result {
                Ok(json_data) => Ok(json_data.into_py(py)),
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// List aliases (simplified version for Python)
    fn list_aliases(&self, alias_type: &str, scope: Option<&str>) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let alias_type = alias_type.to_string();
        
        Python::with_gil(|py| {
            let result: Result<serde_json::Value, eyre::Report> = self.runtime.block_on(async move {
                match alias_type.as_str() {
                    "context" => {
                        let data = inner.list_aliases::<ContextId>(None).await?;
                        serde_json::to_value(data).map_err(|e| eyre::eyre!(e.to_string()))
                    },
                    "identity" => {
                        let context_scope = if let Some(s) = scope {
                            match ContextId::from_str(s) {
                                Ok(id) => Some(id),
                                Err(e) => return Err(eyre::eyre!("Invalid context ID in scope: {}", e))
                            }
                        } else {
                            None
                        };
                        let data = inner.list_aliases::<PublicKey>(context_scope).await?;
                        serde_json::to_value(data).map_err(|e| eyre::eyre!(e.to_string()))
                    },
                    "application" => {
                        let data = inner.list_aliases::<ApplicationId>(None).await?;
                        serde_json::to_value(data).map_err(|e| eyre::eyre!(e.to_string()))
                    },
                    _ => Err(eyre::eyre!("Unsupported alias type: {}", alias_type))
                }
            });
            
            match result {
                Ok(json_data) => Ok(json_data.into_py(py)),
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// Lookup an alias (simplified version for Python)
    fn lookup_alias(&self, alias: &str, alias_type: &str, scope: Option<&str>) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let alias_str = alias.to_string();
        let alias_type = alias_type.to_string();
        
        Python::with_gil(|py| {
            let result: Result<serde_json::Value, eyre::Report> = self.runtime.block_on(async move {
                match alias_type.as_str() {
                    "context" => {
                        let alias_instance = match Alias::<ContextId>::from_str(&alias_str) {
                            Ok(alias) => alias,
                            Err(e) => return Err(eyre::eyre!("Invalid context alias: {}", e))
                        };
                        let data = inner.lookup_alias(alias_instance, None).await?;
                        serde_json::to_value(data).map_err(|e| eyre::eyre!(e.to_string()))
                    },
                    "identity" => {
                        let alias_instance = match Alias::<PublicKey>::from_str(&alias_str) {
                            Ok(alias) => alias,
                            Err(e) => return Err(eyre::eyre!("Invalid identity alias: {}", e))
                        };
                        let context_scope = if let Some(s) = scope {
                            match ContextId::from_str(s) {
                                Ok(id) => Some(id),
                                Err(e) => return Err(eyre::eyre!("Invalid context ID in scope: {}", e))
                            }
                        } else {
                            None
                        };
                        let data = inner.lookup_alias(alias_instance, context_scope).await?;
                        serde_json::to_value(data).map_err(|e| eyre::eyre!(e.to_string()))
                    },
                    "application" => {
                        let alias_instance = match Alias::<ApplicationId>::from_str(&alias_str) {
                            Ok(alias) => alias,
                            Err(e) => return Err(eyre::eyre!("Invalid application alias: {}", e))
                        };
                        let data = inner.lookup_alias(alias_instance, None).await?;
                        serde_json::to_value(data).map_err(|e| eyre::eyre!(e.to_string()))
                    },
                    _ => Err(eyre::eyre!("Unsupported alias type: {}", alias_type))
                }
            });
            
            match result {
                Ok(json_data) => Ok(json_data.into_py(py)),
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// Get supported alias types
    fn get_supported_alias_types(&self) -> PyResult<Vec<String>> {
        Ok(vec![
            "context".to_string(),
            "identity".to_string(), 
            "application".to_string(),
        ])
    }

    /// Get API URL
    fn get_api_url(&self) -> String {
        self.inner.api_url().to_string()
    }

    /// Get application information
    fn get_application(&self, app_id: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let app_id = app_id.parse::<ApplicationId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("Invalid application ID '{}': {}", app_id, e)
            )
        })?;
        
        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                inner.get_application(&app_id).await
            });
            
            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            format!("Failed to serialize response: {}", e)
                        )
                    })?;
                    Ok(json_data.into_py(py))
                },
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// Install development application
    fn install_dev_application(&self, request: &PyAny) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        
        Python::with_gil(|py| {
            let install_request = python_to_install_dev_application_request(py, request)?;
            
            let result = self.runtime.block_on(async move {
                inner.install_dev_application(install_request).await
            });
            
            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            format!("Failed to serialize response: {}", e)
                        )
                    })?;
                    Ok(json_data.into_py(py))
                },
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// Install application
    fn install_application(&self, request: &PyAny) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        
        Python::with_gil(|py| {
            let install_request = python_to_install_application_request(py, request)?;
            
            let result = self.runtime.block_on(async move {
                inner.install_application(install_request).await
            });
            
            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            format!("Failed to serialize response: {}", e)
                        )
                    })?;
                    Ok(json_data.into_py(py))
                },
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// List applications
    fn list_applications(&self) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        
        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                inner.list_applications().await
            });
            
            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            format!("Failed to serialize response: {}", e)
                        )
                    })?;
                    Ok(json_data.into_py(py))
                },
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// Uninstall application
    fn uninstall_application(&self, app_id: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let app_id = app_id.parse::<ApplicationId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("Invalid application ID '{}': {}", app_id, e)
            )
        })?;
        
        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                inner.uninstall_application(&app_id).await
            });
            
            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            format!("Failed to serialize response: {}", e)
                        )
                    })?;
                    Ok(json_data.into_py(py))
                },
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// Delete blob
    fn delete_blob(&self, blob_id: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let blob_id = blob_id.parse::<BlobId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("Invalid blob ID '{}': {}", blob_id, e)
            )
        })?;
        
        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                inner.delete_blob(&blob_id).await
            });
            
            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            format!("Failed to serialize response: {}", e)
                        )
                    })?;
                    Ok(json_data.into_py(py))
                },
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// List blobs
    fn list_blobs(&self) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        
        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                inner.list_blobs().await
            });
            
            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            format!("Failed to serialize response: {}", e)
                        )
                    })?;
                    Ok(json_data.into_py(py))
                },
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// Get blob info
    fn get_blob_info(&self, blob_id: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let blob_id = blob_id.parse::<BlobId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("Invalid blob ID '{}': {}", blob_id, e)
            )
        })?;
        
        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                inner.get_blob_info(&blob_id).await
            });
            
            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            format!("Failed to serialize response: {}", e)
                        )
                    })?;
                    Ok(json_data.into_py(py))
                },
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// Generate context identity
    fn generate_context_identity(&self) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        
        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                inner.generate_context_identity().await
            });
            
            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            format!("Failed to serialize response: {}", e)
                        )
                    })?;
                    Ok(json_data.into_py(py))
                },
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// Get peers count
    fn get_peers_count(&self) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        
        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                inner.get_peers_count().await
            });
            
            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            format!("Failed to serialize response: {}", e)
                        )
                    })?;
                    Ok(json_data.into_py(py))
                },
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// Execute JSON-RPC request
    fn execute_jsonrpc(&self, request: &PyAny) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        
        Python::with_gil(|py| {
            let jsonrpc_request = python_to_jsonrpc_request(py, request)?;
            
            let result = self.runtime.block_on(async move {
                inner.execute_jsonrpc(jsonrpc_request).await
            });
            
            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            format!("Failed to serialize response: {}", e)
                        )
                    })?;
                    Ok(json_data.into_py(py))
                },
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// Grant permissions
    fn grant_permissions(&self, context_id: &str, request: &PyAny) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("Invalid context ID '{}': {}", context_id, e)
            )
        })?;

        Python::with_gil(|py| {
            let permissions = python_to_permissions_request(py, request)?;
            
            let result = self.runtime.block_on(async move {
                inner.grant_permissions(&context_id, permissions).await
            });
            
            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            format!("Failed to serialize response: {}", e)
                        )
                    })?;
                    Ok(json_data.into_py(py))
                },
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// Revoke permissions
    fn revoke_permissions(&self, context_id: &str, request: &PyAny) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("Invalid context ID '{}': {}", context_id, e)
            )
        })?;

        Python::with_gil(|py| {
            let permissions = python_to_permissions_request(py, request)?;
            let result = self.runtime.block_on(async move {
                inner.revoke_permissions(&context_id, permissions).await
            });
            
            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            format!("Failed to serialize response: {}", e)
                        )
                    })?;
                    Ok(json_data.into_py(py))
                },
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// Invite to context
    fn invite_to_context(&self, request: &PyAny) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        
        Python::with_gil(|py| {
            let invite_request = python_to_invite_to_context_request(py, request)?;
            
            let result = self.runtime.block_on(async move {
                inner.invite_to_context(invite_request).await
            });
            
            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            format!("Failed to serialize response: {}", e)
                        )
                    })?;
                    Ok(json_data.into_py(py))
                },
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// Update context application
    fn update_context_application(&self, context_id: &str, request: &PyAny) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("Invalid context ID '{}': {}", context_id, e)
            )
        })?;
        
        Python::with_gil(|py| {
            let update_request = python_to_update_context_application_request(py, request)?;
            
            let result = self.runtime.block_on(async move {
                inner.update_context_application(&context_id, update_request).await
            });
            
            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            format!("Failed to serialize response: {}", e)
                        )
                    })?;
                    Ok(json_data.into_py(py))
                },
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// Get proposal
    fn get_proposal(&self, context_id: &str, proposal_id: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("Invalid context ID '{}': {}", context_id, e)
            )
        })?;
        let proposal_id = proposal_id.parse::<Hash>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("Invalid proposal ID '{}': {}", proposal_id, e)
            )
        })?;
        
        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                inner.get_proposal(&context_id, &proposal_id).await
            });
            
            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            format!("Failed to serialize response: {}", e)
                        )
                    })?;
                    Ok(json_data.into_py(py))
                },
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// Get proposal approvers
    fn get_proposal_approvers(&self, context_id: &str, proposal_id: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("Invalid context ID '{}': {}", context_id, e)
            )
        })?;
        let proposal_id = proposal_id.parse::<Hash>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("Invalid proposal ID '{}': {}", proposal_id, e)
            )
        })?;
        
        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                inner.get_proposal_approvers(&context_id, &proposal_id).await
            });
            
            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            format!("Failed to serialize response: {}", e)
                        )
                    })?;
                    Ok(json_data.into_py(py))
                },
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// List proposals
    fn list_proposals(&self, context_id: &str, args: &PyAny) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("Invalid context ID '{}': {}", context_id, e)
            )
        })?;
        
        Python::with_gil(|py| {
            let args_json = python_to_json(py, args)?;
            
            let result = self.runtime.block_on(async move {
                inner.list_proposals(&context_id, args_json).await
            });
            
            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            format!("Failed to serialize response: {}", e)
                        )
                    })?;
                    Ok(json_data.into_py(py))
                },
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// Get context
    fn get_context(&self, context_id: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("Invalid context ID '{}': {}", context_id, e)
            )
        })?;
        
        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                inner.get_context(&context_id).await
            });
            
            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            format!("Failed to serialize response: {}", e)
                        )
                    })?;
                    Ok(json_data.into_py(py))
                },
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// List contexts
    fn list_contexts(&self) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        
        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                inner.list_contexts().await
            });
            
            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            format!("Failed to serialize response: {}", e)
                        )
                    })?;
                    Ok(json_data.into_py(py))
                },
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// Create context
    fn create_context(&self, request: &PyAny) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        
        Python::with_gil(|py| {
            let create_request = python_to_create_context_request(py, request)?;
            
            let result = self.runtime.block_on(async move {
                inner.create_context(create_request).await
            });
            
            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            format!("Failed to serialize response: {}", e)
                        )
                    })?;
                    Ok(json_data.into_py(py))
                },
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// Delete context
    fn delete_context(&self, context_id: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("Invalid context ID '{}': {}", context_id, e)
            )
        })?;
        
        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                inner.delete_context(&context_id).await
            });
            
            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            format!("Failed to serialize response: {}", e)
                        )
                    })?;
                    Ok(json_data.into_py(py))
                },
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// Sync context
    fn sync_context(&self, context_id: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("Invalid context ID '{}': {}", context_id, e)
            )
        })?;
        
        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                inner.sync_context(&context_id).await
            });
            
            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            format!("Failed to serialize response: {}", e)
                        )
                    })?;
                    Ok(json_data.into_py(py))
                },
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// Join context
    fn join_context(&self, request: &PyAny) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        
        Python::with_gil(|py| {
            let join_request = python_to_join_context_request(py, request)?;
            
            let result = self.runtime.block_on(async move {
                inner.join_context(join_request).await
            });
            
            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            format!("Failed to serialize response: {}", e)
                        )
                    })?;
                    Ok(json_data.into_py(py))
                },
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// Get context storage
    fn get_context_storage(&self, context_id: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("Invalid context ID '{}': {}", context_id, e)
            )
        })?;
        
        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                inner.get_context_storage(&context_id).await
            });
            
            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            format!("Failed to serialize response: {}", e)
                        )
                    })?;
                    Ok(json_data.into_py(py))
                },
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// Get context identities
    fn get_context_identities(&self, context_id: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("Invalid context ID '{}': {}", context_id, e)
            )
        })?;
        
        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                // For now, default to false (all identities, not just owned)
                inner.get_context_identities(&context_id, false).await
            });
            
            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            format!("Failed to serialize response: {}", e)
                        )
                    })?;
                    Ok(json_data.into_py(py))
                },
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
            }
        })
    }

    /// Get context client keys
    fn get_context_client_keys(&self, context_id: &str) -> PyResult<PyObject> {
        let inner = self.inner.clone();
        let context_id = context_id.parse::<ContextId>().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(
                format!("Invalid context ID '{}': {}", context_id, e)
            )
        })?;
        
        Python::with_gil(|py| {
            let result = self.runtime.block_on(async move {
                inner.get_context_client_keys(&context_id).await
            });
            
            match result {
                Ok(data) => {
                    // Convert to JSON first, then to Python
                    let json_data = serde_json::to_value(data).map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                            format!("Failed to serialize response: {}", e)
                        )
                    })?;
                    Ok(json_data.into_py(py))
                },
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    format!("Client error: {}", e)
                )),
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
        // In a real implementation, you might want to use Python's pickle or similar
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

/// Convert a Python object to JSON-serializable value
fn python_to_json(py: Python, obj: &PyAny) -> PyResult<serde_json::Value> {
    if obj.is_none() {
        Ok(serde_json::Value::Null)
    } else if let Ok(b) = obj.extract::<bool>() {
        Ok(serde_json::Value::Bool(b))
    } else if let Ok(i) = obj.extract::<i64>() {
        Ok(serde_json::Value::Number(i.into()))
    } else if let Ok(f) = obj.extract::<f64>() {
        if let Some(n) = serde_json::Number::from_f64(f) {
            Ok(serde_json::Value::Number(n))
        } else {
            Ok(serde_json::Value::String(f.to_string()))
        }
    } else if let Ok(s) = obj.extract::<String>() {
        Ok(serde_json::Value::String(s))
    } else if let Ok(list) = obj.downcast::<PyList>() {
        let mut json_list = Vec::new();
        for item in list.iter() {
            json_list.push(python_to_json(py, item)?);
        }
        Ok(serde_json::Value::Array(json_list))
    } else if let Ok(dict) = obj.downcast::<PyDict>() {
        let mut json_obj = serde_json::Map::new();
        for (key, value) in dict.iter() {
            let key_str = key.extract::<String>()?;
            let json_value = python_to_json(py, value)?;
            json_obj.insert(key_str, json_value);
        }
        Ok(serde_json::Value::Object(json_obj))
    } else {
        // Fallback: convert to string
        Ok(serde_json::Value::String(obj.str()?.to_string()))
    }
}

/// Convert a Python object to JSON-serializable value
trait IntoPyJson {
    fn into_py(self, py: Python<'_>) -> PyObject;
}

impl IntoPyJson for serde_json::Value {
    fn into_py(self, py: Python<'_>) -> PyObject {
        match self {
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
                for item in arr.into_iter() {
                    list.append(item.into_py(py)).unwrap();
                }
                list.into_py(py)
            }
            serde_json::Value::Object(obj) => {
                let dict = PyDict::new(py);
                for (k, v) in obj {
                    dict.set_item(k, v.into_py(py)).unwrap();
                }
                dict.into_py(py)
            }
        }
    }
}
