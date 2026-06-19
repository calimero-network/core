//! A `#[app::view]` method is read-only and must not take `&mut self`.

use calimero_sdk::app;

#[app::state]
struct S;

#[app::logic]
impl S {
    #[app::init]
    pub fn init() -> S {
        S
    }

    #[app::view]
    pub fn bad(&mut self) {}
}

fn main() {}
