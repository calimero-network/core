//! A team planner that brings its own calendar: its release `uses` the calendar with
//! `mode = "create"`, so creating a planner first creates a calendar in the same organization,
//! owned by the same person, and links it under `calendar`.
//!
//! The planner calls that calendar **as itself** (`Caller::App`): the events are the planner's
//! own, shared by everyone who uses this planner. A member's personal calendar is a different
//! app; see `apps/booking` for an app that works on one as the person.
//!
//! `init` is told the members the node created (`links`), and any method reads one with
//! `env::linked`, here through the client `build.rs` generates from the calendar's vendored
//! ABI (`uses::calendar`): its methods are typed, so a misspelled method or a wrong argument
//! does not compile. Upgrading the planner with `upgrade-unit` (or the consent page's
//! `upgradeUnit`) moves its calendar along, within `^0.1`.

use std::collections::BTreeMap;

use calimero_sdk::prelude::*;

// `uses::calendar`: the typed client for the calendar, from its ABI vendored in
// `calimero/abi/calendar.json` (`build.rs`).
include!(concat!(env!("OUT_DIR"), "/uses.rs"));

#[app::state(version = 1)]
pub struct Planner {
    /// How many events members scheduled through this planner.
    scheduled: Counter,
}

#[app::error]
pub enum Error {
    /// No calendar is linked: the planner was not created from its release.
    NoCalendar,
}

/// The calendar this planner's instance was created with.
fn calendar() -> Result<uses::calendar::Client, Error> {
    uses::calendar::Client::linked().ok_or(Error::NoCalendar)
}

#[app::logic]
impl Planner {
    /// `links` names the apps the node created with this instance (`calendar`); the planner
    /// reads them again with `env::linked` whenever it calls, so it keeps nothing here.
    #[app::init]
    pub fn init(links: Option<BTreeMap<String, String>>) -> app::Result<Self> {
        let _created = links;
        Ok(Self::load())
    }

    /// Puts an event on the team's calendar, as the planner. The calendar commits it in its
    /// own log first: if this run failed after that, the event would stay.
    pub fn schedule(&mut self, title: String, at: String) -> app::Result<()> {
        calendar()?.add_event(&title, &at, None)?;
        let _total = self.scheduled.increment()?;
        Ok(())
    }

    /// The team's agenda: the planner's own events on its calendar.
    #[app::view]
    pub fn agenda(&self) -> app::Result<Vec<uses::calendar::Event>> {
        Ok(calendar()?.my_events()?)
    }
}
