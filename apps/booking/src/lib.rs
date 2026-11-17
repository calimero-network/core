//! A booking page that works on a calendar its owner already runs: its release `uses` the
//! calendar with `mode = "attach"`, and the owner points `calendar` at one
//! (`PUT /apps/{booking}/links/calendar`, with their passkey).
//!
//! The reference also lists `methods = ["addEvent", "myEvents"]`: once a person grants it
//! (their app's frontend asks on the consent page, `delegate { app, target }`), booking's
//! calls of those methods reach the calendar **as that person**, so a booking lands in their
//! own calendar and the free slots are read from it. The calendar is not changed for this: it
//! sees its own user, and reads `env::via()` to keep their private events to themselves.
//! Without a grant the same calls run as booking itself, on booking's own events there.
//!
//! The grant covers a person's own calls of `book` and `busy`: a timer or another app's call
//! offers no one onward, so it would run as booking.

use calimero_sdk::prelude::*;

/// An event, as the calendar's `myEvents` answers it.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Event {
    pub title: String,
    pub at: String,
    pub private: bool,
}

/// The calendar's `addEvent` arguments.
#[derive(serde::Serialize)]
struct AddEvent<'a> {
    title: &'a str,
    at: &'a str,
}

#[derive(serde::Serialize)]
struct NoArgs {}

#[app::state(version = 1)]
pub struct Booking {
    /// The slots this page offers, as the calendar writes times.
    slots: Cell<Vec<String>>,
}

#[app::error]
pub enum Error {
    /// The owner has not attached a calendar yet.
    NoCalendar,
    /// Only the booking page's owner sets its slots.
    OwnerOnly,
    /// That slot is not offered, or the person is already busy then.
    SlotTaken { at: String },
}

/// The calendar the owner attached, read on every call: they may attach another one later.
fn calendar() -> app::Result<String> {
    env::linked("calendar").ok_or_else(|| Error::NoCalendar.into())
}

/// When the person (or, without a grant, booking itself) already has something.
fn busy_times(calendar: &str) -> app::Result<Vec<String>> {
    let events: Vec<Event> = env::xcall_view(calendar, "myEvents", &NoArgs {})?;
    Ok(events.into_iter().map(|e| e.at).collect())
}

#[app::logic]
impl Booking {
    #[app::init]
    pub fn init() -> app::Result<Self> {
        Ok(Self::load())
    }

    /// Sets the slots the page offers. The owner's.
    pub fn set_slots(&mut self, slots: Vec<String>) -> app::Result<()> {
        if env::caller() != env::Caller::Account(env::app_owner()) {
            app::bail!(Error::OwnerOnly);
        }
        self.slots.set(&slots)?;
        Ok(())
    }

    /// Books `at` for the caller: an event in their own calendar (with their grant).
    pub fn book(&mut self, at: String, title: String) -> app::Result<()> {
        let calendar = calendar()?;
        let offered = self.slots.get()?.unwrap_or_default().contains(&at);
        if !offered || busy_times(&calendar)?.contains(&at) {
            app::bail!(Error::SlotTaken { at });
        }
        let _done: Option<()> = env::xcall_call(
            &calendar,
            "addEvent",
            &AddEvent {
                title: &title,
                at: &at,
            },
        )?;
        Ok(())
    }

    /// The offered slots the caller is still free at.
    #[app::view]
    pub fn free_slots(&self) -> app::Result<Vec<String>> {
        let busy = busy_times(&calendar()?)?;
        let slots = self.slots.get()?.unwrap_or_default();
        Ok(slots.into_iter().filter(|s| !busy.contains(s)).collect())
    }
}
