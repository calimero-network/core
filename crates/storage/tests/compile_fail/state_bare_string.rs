// Rejection: bare `String` as a state field — has no merge semantics.

use calimero_sdk::app;

#[app::state]
pub struct BadState {
    name: String,
}

fn main() {}
