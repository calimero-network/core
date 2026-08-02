//! Public method names in the SDK's reserved `__calimero` namespace are rejected.

use calimero_sdk::app;

#[app::state]
struct S;

#[app::logic]
impl S {
    #[app::init]
    pub fn init() -> S {
        S
    }

    pub fn __calimero_internal(&self) {}

    // The manifest entry point `#[app::logic]` generates.
    pub fn __calimero_abi(&self) {}
}

fn main() {}
