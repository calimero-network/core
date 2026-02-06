//! Test error message for private event

use calimero_sdk::app;

#[app::event]
enum PrivateEvent {
    Something,
}

fn main() {}
