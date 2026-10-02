//! A read-only method has nothing for an event to run.

use calimero_sdk::app;

#[app::state]
struct S;

#[app::logic]
impl S {
    #[app::init]
    pub fn init() -> S {
        S
    }

    #[app::handler]
    pub fn peek(&self) {}
}

fn main() {}
