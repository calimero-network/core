//! Test error message for explicit ABI specification

use calimero_sdk::app;

#[app::state]
struct MyState;

#[app::logic]
impl MyState {
    pub extern "C" fn method(&self) {}
}

fn main() {}
