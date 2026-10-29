use calimero_app_sdk::prelude::*;

#[app::event]
pub enum Event {
    Booked { id: String },
    Released(String, u32),
}

fn main() {}
