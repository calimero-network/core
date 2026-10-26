use calimero_sdk::app;

#[app::event]
pub enum Event {
    Created,
    ValueSet { key: String, value: String },
}

fn main() {}
