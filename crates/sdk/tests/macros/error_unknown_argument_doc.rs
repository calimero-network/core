//! A `# Arguments` entry must name a parameter of the method it documents.

use calimero_sdk::app;

#[app::state]
struct S;

#[app::logic]
impl S {
    #[app::init]
    pub fn init() -> S {
        S
    }

    /// Stores a value.
    ///
    /// # Arguments
    /// * `vaule` - the value to store.
    pub fn set(&mut self, value: u32) {
        let _ = value;
    }
}

fn main() {}
