use calimero_app_sdk::prelude::*;

#[derive(Record)]
#[table(key = "id", partition = "org")]
pub struct Doc {
    pub id: String,
    pub org: String,
}

fn main() {}
