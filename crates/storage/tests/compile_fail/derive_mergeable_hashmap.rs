// Rejection: `#[derive(Mergeable)]` on a struct with a `HashMap` field. The
// derive applies the same lint as `#[app::state]`.

use std::collections::HashMap;

use calimero_sdk::app::Mergeable;

#[derive(Mergeable)]
pub struct BadStruct {
    items: HashMap<String, String>,
}

fn main() {}
