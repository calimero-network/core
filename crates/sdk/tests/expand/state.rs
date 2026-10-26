use calimero_sdk::app;
use calimero_storage::collections::{LwwRegister, UnorderedMap};

#[app::event]
pub enum Event {
    Touched,
}

#[app::state(emits = Event)]
struct State {
    items: UnorderedMap<String, LwwRegister<String>>,
    label: LwwRegister<String>,
}

fn main() {}
