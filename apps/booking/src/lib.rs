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
//! The calendar is called through `uses::calendar`, the client `build.rs` generates from its
//! vendored ABI: typed methods (futures, awaited in `async fn` methods), and the calendar's
//! own errors as an enum
//! (`uses::calendar::Error::PrivateThroughApp`).
//!
//! The grant covers a person's own calls of `book` and `busy`: a timer or another app's call
//! offers no one onward, so it would run as booking.

use calimero_sdk::prelude::*;

// `uses::calendar()`: the typed client for the calendar, from its vendored ABI. The
// reference lists `methods`, so it has those two: `add_event` and `my_events`.
app::uses!();

#[app::state(version = 1)]
pub struct Booking {
    /// The slots this page offers, as the calendar writes times, in time order: checking
    /// one is a single read.
    slots: Set<String>,
}

#[app::error]
pub enum Error {
    /// Only the booking page's owner sets its slots.
    OwnerOnly,
    /// That slot is not offered, or the person is already busy then.
    SlotTaken { at: String },
    /// A private booking made through booking: the calendar keeps private events to the
    /// person's own calls (`PrivateThroughApp`).
    PrivateNotAllowed,
}

/// When the person (or, without a grant, booking itself) already has something: the
/// calendar's events, typed from its ABI (`uses::calendar::Event`).
async fn busy_times(calendar: &uses::calendar::Client) -> app::Result<Vec<String>> {
    Ok(calendar
        .my_events()
        .await?
        .into_iter()
        .map(|e| e.at)
        .collect())
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
        let _removed = self.slots.clear()?;
        let _added = self.slots.extend(&slots)?;
        Ok(())
    }

    /// Books `at` for the caller: an event in their own calendar (with their grant), private
    /// when `private` says so. The calendar commits it in its own log first: if this run
    /// failed after that, the event would stay.
    pub async fn book(
        &mut self,
        at: String,
        title: String,
        private: Option<bool>,
    ) -> app::Result<()> {
        // The calendar the owner attached, read on every call: they may attach another later.
        let calendar = uses::calendar()?;
        let offered = self.slots.contains(&at)?;
        if !offered || busy_times(&calendar).await?.contains(&at) {
            app::bail!(Error::SlotTaken { at });
        }
        match calendar.add_event(&title, &at, private).await {
            Ok(()) => Ok(()),
            // The calendar's own error, typed: answered as booking's.
            Err(uses::calendar::Error::PrivateThroughApp) => app::bail!(Error::PrivateNotAllowed),
            // Anything else passes on as the calendar answered it (`XcallFailed`).
            Err(other) => Err(other.into()),
        }
    }

    /// The offered slots the caller is still free at.
    #[app::view]
    pub async fn free_slots(&self) -> app::Result<Vec<String>> {
        let busy = busy_times(&uses::calendar()?).await?;
        let mut free = Vec::new();
        for slot in self.slots.iter() {
            let slot = slot?;
            if !busy.contains(&slot) {
                free.push(slot);
            }
        }
        Ok(free)
    }
}
