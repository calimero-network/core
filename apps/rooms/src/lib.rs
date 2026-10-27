//! Rooms: book meeting rooms in 15-minute slots.
//!
//! No double bookings (one `Slot` record per room and slot, its key is the lock), a timer that
//! releases bookings nobody checked in to, a step-up for admin releases, views that hide
//! titles, org roles and per-audience events. See `README.md` for the ABI.

use calimero_app_sdk::env::OrgRole;
use calimero_app_sdk::prelude::*;

/// One slot, in milliseconds.
pub const SLOT_MS: u64 = 900_000;
/// How long after its start a booking may stay unchecked before the timer releases it.
pub const CHECK_IN_GRACE_MS: u64 = 900_000;
/// The most slots one booking may span.
pub const MAX_SLOTS: u32 = 8;
/// The most bookings one timer tick releases; the rest carry over to the next tick.
pub const MAX_RELEASES_PER_TICK: u32 = 500;
/// The length of the window `availability` reports.
pub const DAY_MS: u64 = 86_400_000;

#[derive(Record)]
#[table(key = "id")]
pub struct Room {
    pub id: String,
    pub name: String,
    pub capacity: u32,
}

/// A booking. `end` is exclusive.
#[derive(Record)]
#[table(
    key = "id",
    index(pending_by_start = ["checked_in", "start"]),
    index(by_holder = ["holder", "start"]),
)]
pub struct Booking {
    pub id: String,
    pub room_id: String,
    pub holder: String,
    pub start: u64,
    pub end: u64,
    pub title: String,
    pub checked_in: bool,
}

/// One held slot of one room; its key (room, start) is what makes a double booking impossible.
#[derive(Record)]
#[table(key = ["room_id", "start"], index(by_room = ["room_id", "start"]))]
pub struct Slot {
    pub room_id: String,
    pub start: u64,
    pub booking_id: String,
    pub holder: String,
}

/// A held slot as `availability` reports it; `title` only for the holder and org admins.
#[derive(Record)]
#[table(key = "start")]
pub struct SlotView {
    pub start: u64,
    pub end: u64,
    pub booking_id: String,
    pub title: Option<String>,
    pub mine: bool,
}

#[app::state(version = 1)]
pub struct Rooms {
    rooms: Table<Room>,
    bookings: Table<Booking>,
    slots: Table<Slot>,
}

#[app::error]
pub enum Error {
    LoginRequired,
    Forbidden,
    NotFound,
    Taken { start: u64 },
    Invalid { reason: String },
    Duplicate { index: String },
}

#[app::event]
pub enum Event {
    /// To the org: these slots of the room are gone.
    SlotsTaken {
        room_id: String,
        start: u64,
        end: u64,
    },
    /// To the booker.
    Booked { id: String, title: String },
    /// To the holder.
    BookingReleased { id: String, reason: String },
}

#[app::logic]
impl Rooms {
    #[app::init]
    pub fn init() -> app::Result<Self> {
        Ok(Self::load())
    }

    /// Adds a room (admin); `Duplicate` when the id exists.
    pub fn add_room(&mut self, id: String, name: String, capacity: u32) -> app::Result<()> {
        guards::require_admin()?;
        if id.is_empty() || name.is_empty() || capacity == 0 {
            app::bail!(Error::Invalid {
                reason: "a room needs an id, a name and a capacity".into()
            });
        }
        if self.rooms.contains(&id)? {
            app::bail!(Error::Duplicate { index: "id".into() });
        }
        self.rooms.insert(Room { id, name, capacity })?;
        Ok(())
    }

    /// Books `slot_count` consecutive slots from `start` (member); returns the booking id.
    pub fn book(
        &mut self,
        room_id: String,
        start: u64,
        slot_count: u32,
        title: String,
    ) -> app::Result<String> {
        let me = guards::require_member()?;
        if !start.is_multiple_of(SLOT_MS) {
            app::bail!(Error::Invalid {
                reason: "start must be a multiple of the slot length".into()
            });
        }
        if !(1..=MAX_SLOTS).contains(&slot_count) {
            app::bail!(Error::Invalid {
                reason: "slotCount must be between 1 and 8".into()
            });
        }
        if !self.rooms.contains(&room_id)? {
            app::bail!(Error::NotFound);
        }
        let end = start + u64::from(slot_count) * SLOT_MS;
        let slot_starts: Vec<u64> = (start..end).step_by(SLOT_MS as usize).collect();
        // Check every slot first, so `Taken` names the first held one before anything is written.
        for &slot_start in &slot_starts {
            if self.slots.contains(&(room_id.as_str(), slot_start))? {
                app::bail!(Error::Taken { start: slot_start });
            }
        }
        let id = format!("b{}", env::unique_id());
        for slot_start in slot_starts {
            self.slots.insert(Slot {
                room_id: room_id.clone(),
                start: slot_start,
                booking_id: id.clone(),
                holder: me.clone(),
            })?;
        }
        self.bookings.insert(Booking {
            id: id.clone(),
            room_id: room_id.clone(),
            holder: me.clone(),
            start,
            end,
            title: title.clone(),
            checked_in: false,
        })?;
        env::emit(
            Event::SlotsTaken {
                room_id,
                start,
                end,
            },
            Audience::Org,
        )?;
        env::emit(
            Event::Booked {
                id: id.clone(),
                title,
            },
            Audience::Accounts(vec![me]),
        )?;
        Ok(id)
    }

    /// Marks the booking as used, so the timer keeps it (holder only).
    pub fn check_in(&mut self, id: String) -> app::Result<()> {
        let me = guards::require_member()?;
        self.held_by(&id, &me)?;
        self.bookings.update(&id, |b| b.checked_in = true)?;
        Ok(())
    }

    /// Frees the booking's slots (holder only).
    pub fn cancel(&mut self, id: String) -> app::Result<()> {
        let me = guards::require_member()?;
        let booking = self.held_by(&id, &me)?;
        self.remove_booking(&booking)?;
        Ok(())
    }

    /// Frees any booking; the holder is told why. Needs a fresh passkey confirmation.
    #[app::confirm]
    pub fn admin_release(&mut self, id: String, reason: String) -> app::Result<()> {
        guards::require_admin()?;
        let Some(booking) = self.bookings.get(&id)? else {
            app::bail!(Error::NotFound);
        };
        self.release(&booking, &reason)
    }

    /// Releases bookings nobody checked in to, `CHECK_IN_GRACE_MS` after their start.
    #[app::every("1m")]
    #[app::allow(system)]
    pub fn release_no_shows(&mut self) -> app::Result<()> {
        let now = env::time_now();
        // A booking is due when start + grace <= now, i.e. start < now - grace + 1.
        let Some(horizon) = now.checked_sub(CHECK_IN_GRACE_MS) else {
            return Ok(());
        };
        let due = self.bookings.find(
            "pendingByStart",
            &[KeyPart::Bool(false)],
            FindOpts::default()
                .to(vec![KeyPart::Int(i128::from(horizon) + 1)])
                .limit(MAX_RELEASES_PER_TICK),
        )?;
        for booking in due.items {
            self.release(&booking, "no-show")?;
        }
        Ok(())
    }

    /// The held slots of `room_id` in the 24 hours from `day_start`; titles only for the
    /// holder and org admins.
    #[app::view]
    pub fn availability(&self, room_id: String, day_start: u64) -> app::Result<Vec<SlotView>> {
        let me = guards::require_member()?;
        let admin = is_admin(&me);
        let page = self.slots.find(
            "byRoom",
            &[KeyPart::Str(room_id)],
            FindOpts::default()
                .from(vec![KeyPart::Int(i128::from(day_start))])
                .to(vec![KeyPart::Int(
                    i128::from(day_start) + i128::from(DAY_MS),
                )])
                .limit(100),
        )?;
        let mut out = Vec::with_capacity(page.items.len());
        for slot in page.items {
            let mine = slot.holder == me;
            let title = if mine || admin {
                self.bookings.get(&slot.booking_id)?.map(|b| b.title)
            } else {
                None
            };
            out.push(SlotView {
                start: slot.start,
                end: slot.start + SLOT_MS,
                booking_id: slot.booking_id,
                title,
                mine,
            });
        }
        Ok(out)
    }

    /// The caller's bookings, earliest first.
    #[app::view]
    pub fn my_bookings(&self) -> app::Result<Vec<Booking>> {
        let me = guards::require_member()?;
        let page = self.bookings.find(
            "byHolder",
            &[KeyPart::Str(me)],
            FindOpts::default().limit(100),
        )?;
        Ok(page.items)
    }

    /// One booking; `NotFound` unless the caller holds it or is an admin.
    #[app::view]
    pub fn booking(&self, id: String) -> app::Result<Booking> {
        let me = guards::require_member()?;
        match self.bookings.get(&id)? {
            Some(b) if b.holder == me || is_admin(&me) => Ok(b),
            _ => Err(Error::NotFound.into()),
        }
    }

    /// The booking `id` if `me` holds it, else `NotFound` (hides existence).
    fn held_by(&self, id: &str, me: &str) -> app::Result<Booking> {
        match self.bookings.get(id)? {
            Some(b) if b.holder == me => Ok(b),
            _ => Err(Error::NotFound.into()),
        }
    }

    /// Frees the slots and drops the booking.
    fn remove_booking(&mut self, booking: &Booking) -> app::Result<()> {
        for slot_start in (booking.start..booking.end).step_by(SLOT_MS as usize) {
            self.slots.remove(&(booking.room_id.as_str(), slot_start))?;
        }
        self.bookings.remove(&booking.id)?;
        Ok(())
    }

    /// Frees the booking and tells its holder why.
    fn release(&mut self, booking: &Booking, reason: &str) -> app::Result<()> {
        self.remove_booking(booking)?;
        env::emit(
            Event::BookingReleased {
                id: booking.id.clone(),
                reason: reason.to_owned(),
            },
            Audience::Accounts(vec![booking.holder.clone()]),
        )?;
        Ok(())
    }
}

fn is_admin(account: &str) -> bool {
    env::org_role(account).is_some_and(|r| r >= OrgRole::Admin)
}
