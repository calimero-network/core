//! A team planner that brings its own calendar: its release `uses` the calendar with
//! `mode = "create"`, so creating a planner first creates a calendar in the same organization,
//! owned by the same person, and links it under `calendar`.
//!
//! The planner calls that calendar **as itself** (`Caller::App`): the events are the planner's
//! own, shared by everyone who uses this planner. A member's personal calendar is a different
//! app; see `apps/booking` for an app that works on one as the person.
//!
//! `init` is told the members the node created (`links`), and any method reads one with
//! `env::linked`. Upgrading the planner with `upgrade-unit` (or the consent page's
//! `upgradeUnit`) moves its calendar along, within `^0.1`.

use std::collections::BTreeMap;

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
pub struct Planner {
    /// How many events members scheduled through this planner.
    scheduled: Cell<u64>,
}

#[app::error]
pub enum Error {
    /// No calendar is linked: the planner was not created from its release.
    NoCalendar,
}

/// The calendar this planner's instance was created with.
fn calendar() -> app::Result<String> {
    env::linked("calendar").ok_or_else(|| Error::NoCalendar.into())
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

    /// Puts an event on the team's calendar, as the planner.
    pub fn schedule(&mut self, title: String, at: String) -> app::Result<()> {
        let _done: Option<()> = env::xcall_call(
            &calendar()?,
            "addEvent",
            &AddEvent {
                title: &title,
                at: &at,
            },
        )?;
        self.scheduled
            .set(&(self.scheduled.get()?.unwrap_or(0) + 1))?;
        Ok(())
    }

    /// The team's agenda: the planner's own events on its calendar.
    #[app::view]
    pub fn agenda(&self) -> app::Result<Vec<Event>> {
        env::xcall_view(&calendar()?, "myEvents", &NoArgs {})
    }
}
