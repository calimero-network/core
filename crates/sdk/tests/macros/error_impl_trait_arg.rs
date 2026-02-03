//! Test error message for impl Trait arguments (covered by invalid_args.rs but let's be explicit)

use calimero_sdk::app;
use std::fmt::Display;

#[app::state]
struct MyState;

#[app::logic]
impl MyState {
    pub fn method(&self, value: impl Display) {
        let _ = value.to_string();
    }
}

fn main() {}
