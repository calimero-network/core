use calimero_sdk::prelude::*;

#[app::error]
pub enum Error {
    NotFound,
    Invalid(String),
}

fn main() {}
