//! A handler that is not `pub` is never exported, so no peer could run it.

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
    fn on_event(&mut self) {}
}

fn main() {}
