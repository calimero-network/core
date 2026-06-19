//! Valid method-level generics and lifetimes accepted by `#[app::logic]`.
//!
//! State itself takes no generics or lifetimes (both are rejected by
//! `#[app::state]`); the test target is generic/lifetime parameters declared on
//! the *methods*, including a parameter named `calimero` that brushes up against
//! the SDK namespace without colliding with a reserved identifier.

use calimero_sdk::app;

#[app::state]
struct MyType;

#[app::logic]
impl MyType {
    #[app::init]
    pub fn init() -> MyType {
        MyType
    }

    // ignored because it's private — still parsed by the macro
    fn method0<'k, K, 'v, V, 'c>(&self, tag: &str, key: &'k K, value: &'v V, calimero: &'c str) {}

    pub fn method<'k, 'v>(&self, tag: &str, key: &'k str, value: &'v str) {}
}

fn main() {}
