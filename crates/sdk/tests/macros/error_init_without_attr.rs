//! Test error message for method named "init" without #[app::init] attribute

use calimero_sdk::app;

#[app::state]
struct MyState;

#[app::logic]
impl MyState {
    pub fn init() -> Self {
        Self
    }
}

fn main() {}
