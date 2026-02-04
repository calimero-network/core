//! Tests for invalid nested generic patterns that should produce compile errors

use calimero_sdk::app;

#[app::state]
struct MyState<T>(T);

#[app::logic]
impl<T> MyState<T> {
    pub fn method(&self) {}
}

fn main() {}
