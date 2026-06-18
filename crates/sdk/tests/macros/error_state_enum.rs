//! `#[app::state]` on an enum must be rejected — enums have no canonical merge.

use calimero_sdk::app;

#[app::state]
pub enum BadState {
    A,
    B,
}

fn main() {}
