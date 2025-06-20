use std::any::Any;
use std::collections::HashMap;
use std::sync::{Mutex, Once};

use lazy_static::lazy_static;
use serde_json::Value;

use crate::AuthError;

/// Trait for authentication data types
pub trait AuthDataType: Send + Sync {
    /// Get the method name this auth data is for
    fn method_name(&self) -> &str;

    /// Parse from JSON value
    fn parse_from_value(&self, value: Value) -> eyre::Result<Box<dyn Any + Send + Sync>>;

    /// Get a sample structure for documentation
    fn get_sample_structure(&self) -> Value;
}

// Global registry for auth data types
lazy_static! {
    static ref AUTH_DATA_REGISTRY: Mutex<AuthDataRegistry> = Mutex::new(AuthDataRegistry::new());
}

/// Registry for auth data types
struct AuthDataRegistry {
    types: HashMap<String, Box<dyn AuthDataType>>,
}

impl AuthDataRegistry {
    fn new() -> Self {
        Self {
            types: HashMap::new(),
        }
    }

    fn register(&mut self, auth_type: Box<dyn AuthDataType>) {
        let method = auth_type.method_name().to_string();
        self.types.insert(method, auth_type);
    }

    fn get(&self, method: &str) -> Option<&Box<dyn AuthDataType>> {
        self.types.get(method)
    }

    fn get_all_methods(&self) -> Vec<String> {
        self.types.keys().cloned().collect()
    }
}

/// Register an auth data type
pub fn register_auth_data_type(auth_type: Box<dyn AuthDataType>) {
    let mut registry = AUTH_DATA_REGISTRY.lock().unwrap();
    registry.register(auth_type);
}

/// Parse auth data from a JSON value
pub fn parse_auth_data(method: &str, value: Value) -> eyre::Result<Box<dyn Any + Send + Sync>> {
    let registry = AUTH_DATA_REGISTRY.lock().unwrap();

    if let Some(auth_type) = registry.get(method) {
        auth_type.parse_from_value(value)
    } else {
        Err(eyre::eyre!(
            "Unsupported authentication method: {}. Supported methods: {}",
            method,
            registry.get_all_methods().join(", ")
        ))
    }
}

/// Get information about all registered auth data types
pub fn get_all_auth_data_types() -> HashMap<String, Value> {
    let registry = AUTH_DATA_REGISTRY.lock().unwrap();

    registry
        .types
        .iter()
        .map(|(method, auth_type)| (method.clone(), auth_type.get_sample_structure()))
        .collect()
}

/// Macro for registering auth data types
#[macro_export]
macro_rules! register_auth_data_type {
    ($auth_type:expr) => {
        #[ctor::ctor]
        fn register_this_auth_data_type() {
            $crate::providers::core::provider_data_registry::register_auth_data_type(Box::new(
                $auth_type,
            ));
        }
    };
}
