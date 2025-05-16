// Core provider functionality
pub mod provider;

// Provider registration system
pub mod provider_registry;

// Auth data type system
pub mod provider_data_registry;

// Re-export macros for convenient usage
#[macro_use]
pub mod macros;

// Re-export key traits and types
pub use provider::{AuthProvider, AuthRequestVerifier, AuthVerifierFn};
pub use provider_data_registry::{register_auth_data_type, AuthDataType};
pub use provider_registry::{
    get_all_provider_registrations, register_provider, ProviderRegistration,
};
