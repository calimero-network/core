//! Text inside `# Arguments` that is not an entry or an indented continuation is rejected.

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
    /// * `value` - in cents.
    /// Must be positive.
    pub fn set(&mut self, value: u32) {
        let _ = value;
    }
}

fn main() {}
