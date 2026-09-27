//! A `# Arguments` entry must not name the same parameter twice.

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
    /// * `value` - the value to store.
    /// * `value` - stored as is.
    pub fn set(&mut self, value: u32) {
        let _ = value;
    }
}

fn main() {}
