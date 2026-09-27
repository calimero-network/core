use calimero_sdk::app;
use calimero_sdk::serde::Serialize;

#[derive(Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct Base {
    id: u32,
}

// An event payload is described like any ABI type, so the same serde key is refused.
#[app::event]
pub enum Event {
    Moved {
        #[serde(flatten)]
        base: Base,
        extra: String,
    },
}

fn main() {}
