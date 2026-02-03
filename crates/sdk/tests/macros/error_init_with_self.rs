//! Test error message for init method with self receiver

use calimero_sdk::app;

#[app::state]
struct MyState;

#[app::logic]
impl MyState {
    #[app::init]
    pub fn init(&self) -> Self {
        Self
    }
}

fn main() {}
