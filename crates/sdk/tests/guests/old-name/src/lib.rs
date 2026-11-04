//! Test guest for `tests/names.rs`: an app that depends on the SDK by its old name.

use calimero_app_sdk::prelude::*;

#[derive(Record)]
#[table(key = "id")]
pub struct Item {
    pub id: String,
    pub size: u64,
    pub data: Vec<u8>,
}

#[app::state(version = 1)]
pub struct Store {
    items: Table<Item>,
    puts: Counter,
}

#[app::error]
pub enum Error {
    Missing { id: String },
}

#[app::event]
pub enum Event {
    Put { id: String, size: u64 },
}

#[app::logic]
impl Store {
    #[app::init]
    pub fn init() -> app::Result<Self> {
        Ok(Self::load())
    }

    /// Stores `data` under `id`; `size` is a u64, so a decimal string on the wire.
    pub fn put(&mut self, id: String, size: u64, data: Vec<u8>) -> app::Result<u64> {
        self.items.insert(Item {
            id: id.clone(),
            size,
            data,
        })?;
        env::emit(Event::Put { id, size }, Audience::All)?;
        Ok(self.puts.increment()?)
    }

    /// The stored item; `Missing` when there is none.
    #[app::view]
    pub fn get(&self, id: String) -> app::Result<Item> {
        match self.items.get(&id)? {
            Some(item) => Ok(item),
            None => app::bail!(Error::Missing { id }),
        }
    }
}
