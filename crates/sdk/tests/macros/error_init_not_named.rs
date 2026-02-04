//! Test error message for init method not named "init"

use calimero_sdk::app;

#[app::state]
struct MyState;

#[app::logic]
impl MyState {
    #[app::init]
    pub fn initialize() -> Self {
        Self
    }
}

fn main() {}
