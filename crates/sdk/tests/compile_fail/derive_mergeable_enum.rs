// Rejection: `#[derive(Mergeable)]` on an enum. Variants have no canonical
// merge rule; users should wrap in `LwwRegister<MyEnum>` or impl manually.

use calimero_sdk::app::Mergeable;
use calimero_storage::collections::Counter;

#[derive(Mergeable)]
pub enum BadEnum {
    Empty,
    Counted(Counter),
}

fn main() {}
