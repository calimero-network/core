//! A read-only method has nothing to destroy or repeat.

use calimero_sdk::app;

#[app::state]
struct S;

#[app::logic]
impl S {
    #[app::init]
    pub fn init() -> S {
        S
    }

    #[app::destructive]
    pub fn peek(&self) {}
}

fn main() {}
