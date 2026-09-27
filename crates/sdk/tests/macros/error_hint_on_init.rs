//! An initializer creates the state; neither hint describes it.

use calimero_sdk::app;

#[app::state]
struct S;

#[app::logic]
impl S {
    #[app::init]
    #[app::destructive]
    pub fn init() -> S {
        S
    }
}

fn main() {}
