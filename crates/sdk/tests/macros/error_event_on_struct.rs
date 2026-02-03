//! Test error message for applying #[app::event] to a struct instead of enum

use calimero_sdk::app;

#[app::event]
pub struct InvalidStructEvent {
    key: String,
    value: String,
}

fn main() {}
