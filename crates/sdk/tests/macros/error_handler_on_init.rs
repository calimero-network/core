//! An initializer creates the state; no event runs it.

use calimero_sdk::app;

#[app::state]
struct S;

#[app::logic]
impl S {
    #[app::init]
    #[app::handler]
    pub fn init() -> S {
        S
    }
}

fn main() {}
