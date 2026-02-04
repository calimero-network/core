//! Test error message for const generic parameters in methods

use calimero_sdk::app;

#[app::state]
struct MyState;

#[app::logic]
impl MyState {
    pub fn method<const N: usize>(&self, arr: [u8; N]) -> usize {
        arr.len()
    }
}

fn main() {}
