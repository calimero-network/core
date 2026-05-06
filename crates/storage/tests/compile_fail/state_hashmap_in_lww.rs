// Rejection: `HashMap` smuggled inside `LwwRegister` — would compile-and-blob
// without the lint, defeating CRDT merge semantics. This is the exact bug
// pattern the lint is designed to catch.

use std::collections::HashMap;

use calimero_sdk::app;
use calimero_storage::collections::LwwRegister;

#[app::state]
pub struct BadState {
    items: LwwRegister<HashMap<String, String>>,
}

fn main() {}
