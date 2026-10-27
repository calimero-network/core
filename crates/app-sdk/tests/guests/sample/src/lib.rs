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

#[app::state(version = 3)]
pub struct Rooms {
    bookings: Table<Booking>,
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
