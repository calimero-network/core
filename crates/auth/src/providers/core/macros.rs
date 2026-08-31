/// Macro for registering auth providers
///
/// This macro creates a constructor function that automatically registers
/// the provider with the global registry during program initialization.
#[macro_export]
macro_rules! register_auth_provider {
    ($registration:expr) => {
        // `unsafe` is ctor 1.0's acknowledgement that this runs before `main`,
        // outside any runtime the program has set up. Registration only pushes
        // into a lock-guarded global, which is why it is sound here.
        #[ctor::ctor(unsafe)]
        fn register_this_provider() {
            use std::sync::Arc;
            $crate::providers::core::provider_registry::register_provider(Arc::new($registration));
        }
    };
}
