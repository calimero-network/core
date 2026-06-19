//! Valid method argument shapes accepted by `#[app::logic]`.
//!
//! State carries no fields here — the test target is the argument grammar
//! (refs, lifetimes, owned values, `Self`, slices), not state layout.

use calimero_sdk::app;

#[app::state]
struct MyType;

#[app::logic]
impl MyType {
    #[app::init]
    pub fn init() -> MyType {
        MyType
    }

    pub fn method_00(&self, key: &str, value: &str) {}
    pub fn method_01<'a>(&self, key: &'a str, value: &'a str) {}
    pub fn method_02(&self, key: String, value: Self) {}
    pub fn method_03<'a>(&self, entries: &'a [(&'a str, &'a Self)]) -> usize {
        entries.len()
    }
}

fn main() {}
