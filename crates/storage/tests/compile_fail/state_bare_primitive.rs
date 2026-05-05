// Rejection: bare `u64` as a state field — primitives aren't Mergeable.
// Suggestion should point at `Counter` (or `LwwRegister<T>`).

use calimero_sdk::app;

#[app::state]
pub struct BadState {
    count: u64,
}

fn main() {}
