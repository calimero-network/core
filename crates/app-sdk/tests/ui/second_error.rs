use calimero_app_sdk::prelude::*;

#[app::error]
pub enum Error {
    NotFound,
}

#[app::error]
pub enum MoreErrors {
    Gone,
}

fn main() {}
