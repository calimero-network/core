use std::any::Any;

// No additional imports needed
use serde_json::Value;

use crate::providers::impls::farcaster::FarcasterAuthData;

/// Farcaster authentication data type handler
pub struct FarcasterAuthDataType;

impl FarcasterAuthDataType {
    pub fn new() -> Self {
        Self
    }
}

impl crate::providers::core::provider_data_registry::AuthDataType for FarcasterAuthDataType {
    fn method_name(&self) -> &str {
        "farcaster_jwt"
    }

    fn parse_from_value(&self, value: Value) -> eyre::Result<Box<dyn Any + Send + Sync>> {
        let auth_data: FarcasterAuthData = serde_json::from_value(value)
            .map_err(|e| eyre::eyre!("Failed to parse Farcaster auth data: {}", e))?;

        Ok(Box::new(auth_data))
    }

    fn get_sample_structure(&self) -> Value {
        serde_json::json!({
            "token": "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9...",
            "domain": "your-calimero-domain.com",
            "client_name": "my-app"
        })
    }
}
