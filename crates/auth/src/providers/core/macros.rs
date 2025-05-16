/// Macro for registering auth providers
/// 
/// This macro creates a constructor function that automatically registers
/// the provider with the global registry during program initialization.
#[macro_export]
macro_rules! register_auth_provider {
    ($registration:expr) => {
        #[ctor::ctor]
        fn register_this_provider() {
            use std::sync::Arc;
            $crate::providers::core::provider_registry::register_provider(Arc::new($registration));
        }
    };
} 