//! Test guest: a small rooms app using every `app` attribute, run by `tests/abi.rs`.

use calimero_app_sdk::prelude::*;

#[derive(Record)]
#[table(
    key = "id",
    unique(slot = ["room_id", "start"]),
    index(by_start = ["checked_in", "start"]),
    count(by_room = ["room_id"]),
)]
pub struct Booking {
    pub id: String,
    pub room_id: String,
    pub start: u64,
    pub title: String,
    pub checked_in: bool,
}

/// A record with a bytes field: CBOR bytes when stored, base64url in JSON.
#[derive(Record)]
#[table(key = "id")]
pub struct Blob {
    pub id: String,
    pub data: Vec<u8>,
}

#[app::state(version = 3)]
pub struct Rooms {
    bookings: Table<Booking>,
    blobs: Table<Blob>,
    titles: UnorderedMap<String, String>,
    next_id: Counter,
}

#[app::error]
pub enum Error {
    NotFound,
    Taken { start: u64 },
    Invalid(String),
}

#[app::event]
pub enum Event {
    Booked {
        id: String,
        room_id: String,
        start: u64,
    },
    Released {
        id: String,
    },
    Stored {
        id: String,
        data: Vec<u8>,
    },
}

#[app::logic]
impl Rooms {
    #[app::init]
    pub fn init() -> app::Result<Self> {
        Ok(Self::load())
    }

    /// Books `room_id` at `start`; `Taken` when the slot is held.
    pub fn book(&mut self, room_id: String, start: u64, title: String) -> app::Result<String> {
        if title.is_empty() {
            app::bail!(Error::Invalid("empty title".into()));
        }
        if self.slot_taken(&room_id, start)? {
            app::bail!(Error::Taken { start });
        }
        let id = env::unique_id();
        self.bookings.insert(Booking {
            id: id.clone(),
            room_id: room_id.clone(),
            start,
            title: title.clone(),
            checked_in: false,
        })?;
        self.titles.insert(&id, &title)?;
        env::emit(
            Event::Booked {
                id: id.clone(),
                room_id,
                start,
            },
            Audience::All,
        )?;
        Ok(id)
    }

    /// Whether `room_id` is free at `start`.
    #[app::view]
    pub fn availability(&self, room_id: String, start: u64) -> app::Result<bool> {
        Ok(!self.slot_taken(&room_id, start)?)
    }

    /// Returns `start` unchanged: u64 arguments and results are decimal strings.
    #[app::view]
    pub fn echo_start(&self, start: u64, label: Option<String>) -> app::Result<u64> {
        let _ = label;
        Ok(start)
    }

    /// A booking as stored.
    #[app::view]
    pub fn booking(&self, id: String) -> app::Result<Booking> {
        self.bookings
            .get(&id)?
            .ok_or_else(|| Error::NotFound.into())
    }

    /// Bookings per room, from the maintained count.
    #[app::view]
    #[app::public]
    pub fn room_count(&self, room_id: String) -> app::Result<u64> {
        Ok(self.bookings.count("byRoom", &[KeyPart::Str(room_id)])?)
    }

    /// Removes a booking; needs a fresh passkey confirmation.
    #[app::confirm]
    pub fn admin_release(&mut self, id: String) -> app::Result<()> {
        if !self.bookings.remove(&id)? {
            app::bail!(Error::NotFound);
        }
        env::emit(Event::Released { id }, Audience::All)?;
        Ok(())
    }

    /// Releases every unchecked booking that started before the tick; returns the tick slot.
    #[app::every("1m")]
    #[app::allow(system)]
    pub fn release_no_shows(&mut self) -> app::Result<Option<u64>> {
        let Some(tick) = env::timer() else {
            return Ok(None);
        };
        let due = self.bookings.find(
            "byStart",
            &[KeyPart::Bool(false)],
            FindOpts::default().to(vec![KeyPart::Int(i128::from(tick.scheduled))]),
        )?;
        for b in due.items {
            let _ = self.bookings.remove(&b.id)?;
        }
        Ok(Some(tick.scheduled))
    }

    /// Stores bytes, given as base64url.
    pub fn put_blob(&mut self, id: String, data: Vec<u8>) -> app::Result<()> {
        self.blobs.upsert(Blob {
            id: id.clone(),
            data: data.clone(),
        })?;
        env::emit(Event::Stored { id, data }, Audience::All)?;
        Ok(())
    }

    /// A stored blob record.
    #[app::view]
    pub fn blob(&self, id: String) -> app::Result<Blob> {
        self.blobs.get(&id)?.ok_or_else(|| Error::NotFound.into())
    }

    /// Returns `data` unchanged: bytes are base64url in arguments and results.
    #[app::view]
    pub fn echo_bytes(&self, data: Vec<u8>, extra: Option<Vec<u8>>) -> app::Result<Vec<u8>> {
        let _ = extra;
        Ok(data)
    }

    /// Tells the org that `id` was released.
    pub fn announce(&mut self, id: String) -> app::Result<()> {
        env::emit(Event::Released { id }, Audience::Org)?;
        Ok(())
    }

    /// Tells a team that `id` was released.
    pub fn announce_team(&mut self, team: String, id: String) -> app::Result<()> {
        env::emit(Event::Released { id }, Audience::Team(team))?;
        Ok(())
    }

    /// Tells `accounts` that `id` was released.
    pub fn notify(&mut self, accounts: Vec<String>, id: String) -> app::Result<()> {
        env::emit(Event::Released { id }, Audience::Accounts(accounts))?;
        Ok(())
    }

    /// `len` random bytes (seeded in a logged run).
    pub fn roll(&mut self, len: u64) -> app::Result<Vec<u8>> {
        Ok(env::random_bytes(len as usize))
    }

    /// A fresh unique id.
    pub fn new_id(&mut self) -> app::Result<String> {
        Ok(env::unique_id())
    }

    /// Writes, then fails with an app error.
    pub fn write_then_fail(&mut self) -> app::Result<()> {
        self.titles.insert(&"k".to_owned(), &"v".to_owned())?;
        app::bail!(Error::NotFound);
    }

    /// Writes, then traps.
    pub fn write_then_trap(&mut self) -> app::Result<()> {
        self.titles.insert(&"k".to_owned(), &"v".to_owned())?;
        panic!("boom");
    }

    /// Sets a title (for `flaky`).
    pub fn put_title(&mut self, key: String, value: String) -> app::Result<()> {
        self.titles.insert(&key, &value)?;
        Ok(())
    }

    /// Removes a title.
    pub fn clear_title(&mut self, key: String) -> app::Result<()> {
        self.titles.remove(&key)?;
        Ok(())
    }

    /// The time the host serves.
    #[app::view]
    pub fn now(&self) -> app::Result<u64> {
        Ok(env::time_now())
    }

    /// A timer that fails while the title `fail` is set.
    #[app::every("1m")]
    pub fn flaky(&mut self) -> app::Result<String> {
        if self.titles.get(&"fail".to_owned())?.is_some() {
            app::bail!(Error::Invalid("flaky".into()));
        }
        let Some(tick) = env::timer() else {
            app::bail!(Error::NotFound);
        };
        Ok(match tick.previous {
            Some(p) => format!("{}/{p}", tick.scheduled),
            None => format!("{}/-", tick.scheduled),
        })
    }

    /// Reports the tick it ran for as `<scheduled>/<previous or ->`.
    #[app::every("1m")]
    pub fn tick_info(&mut self) -> app::Result<String> {
        let Some(tick) = env::timer() else {
            app::bail!(Error::NotFound);
        };
        Ok(match tick.previous {
            Some(p) => format!("{}/{p}", tick.scheduled),
            None => format!("{}/-", tick.scheduled),
        })
    }

    /// A timer without `allow(system)`: `every` implies it (Ruling 26).
    #[app::every("30s")]
    pub fn heartbeat(&mut self) -> app::Result<()> {
        Ok(())
    }

    /// The counter, for a call that touches no table.
    pub fn bump(&mut self) -> app::Result<u64> {
        Ok(self.next_id.increment()?)
    }

    fn slot_taken(&self, room_id: &str, start: u64) -> app::Result<bool> {
        let page = self.bookings.find(
            "byStart",
            &[KeyPart::Bool(false), KeyPart::Int(i128::from(start))],
            FindOpts::default().limit(50),
        )?;
        Ok(page.items.iter().any(|b| b.room_id == room_id))
    }
}
