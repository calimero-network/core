use calimero_sdk::prelude::*;

#[derive(serde::Serialize, serde::Deserialize, AbiType)]
pub struct Sizes {
    pub total: u128,
}

#[derive(Record)]
#[table(key = "id")]
pub struct Row {
    pub id: String,
    pub size: usize,
}

#[app::state(version = 1)]
pub struct S {
    n: Counter,
}

#[app::logic]
impl S {
    pub fn take(&mut self, n: isize) -> app::Result<()> {
        let _ = n;
        Ok(())
    }

    #[app::view]
    pub fn count(&self) -> app::Result<usize> {
        Ok(0)
    }
}

fn main() {}
