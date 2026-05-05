// Rejection: `HashMap` as a state field — std collections aren't CRDTs.

use std::collections::HashMap;

use calimero_sdk::app;

#[app::state]
pub struct BadState {
    items: HashMap<String, String>,
}

fn main() {}
