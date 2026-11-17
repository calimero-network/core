//! A calendar: each caller keeps its own events. A person's are keyed by their account (an
//! API key of theirs counts as them), an app's by its id.
//!
//! It is the app others build on in the units examples, and it knows nothing about them:
//!
//! - `apps/planner` creates a calendar with each of its instances (`uses`, `mode = "create"`)
//!   and calls it as itself, so its events are the planner's own (`app:<planner>`).
//! - `apps/booking` is attached to a calendar its owner already runs (`mode = "attach"`) and,
//!   once a person grants it, calls `addEvent` and `myEvents` **as that person**: the events
//!   land in their own calendar, under the same rules as their own calls.
//!
//! The one thing it does differently for a call through another app is read `env::via()`: a
//! plugin acting for a person may not add a private event, and never sees one.
//!
//! Storage: one row per event, keyed `(owner, at, id)`. Adding an event writes that one row
//! (a list per owner would be read and rewritten whole on every add), and a caller's events
//! are the rows under their owner prefix, already in time order.

use calimero_sdk::prelude::*;

/// One event, as `myEvents` lists it.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Event {
    pub title: String,
    /// When, as the caller writes it (an ISO date and time, say).
    pub at: String,
    /// Only the person themselves sees it, never an app acting for them.
    pub private: bool,
}

#[app::state(version = 1)]
pub struct Calendar {
    /// Every event, keyed `(owner, at, id)`: the owner is `account:<id>` or `app:<id>`, and
    /// `id` (`env::unique_id()`) keeps two events at the same time apart.
    events: UnorderedMap<(String, String, String), Event>,
}

#[app::error]
pub enum Error {
    /// The caller is neither a person nor an app.
    UnknownCaller,
    /// An app acting for a person may not add a private event.
    PrivateThroughApp,
}

#[app::event]
pub enum CalendarEvent {
    Added { owner: String, at: String },
}

/// Whose events a call reads and writes: the person (also through an API key or an app they
/// granted), or the app calling as itself.
fn owner() -> app::Result<String> {
    Ok(match env::caller() {
        env::Caller::Account(a) => format!("account:{a}"),
        env::Caller::ApiKey { account, .. } => format!("account:{account}"),
        env::Caller::App(a) => format!("app:{a}"),
        _ => app::bail!(Error::UnknownCaller),
    })
}

#[app::logic]
impl Calendar {
    #[app::init]
    pub fn init() -> app::Result<Self> {
        Ok(Self::load())
    }

    /// Adds an event to the caller's own calendar. Any app may call it as itself
    /// (`allow(app = "*")`); a call another app makes as a person is the person's.
    #[app::allow(app = "*")]
    pub fn add_event(
        &mut self,
        title: String,
        at: String,
        private: Option<bool>,
    ) -> app::Result<()> {
        let private = private.unwrap_or(false);
        if private && env::via().is_some() {
            app::bail!(Error::PrivateThroughApp);
        }
        let owner = owner()?;
        let event = Event {
            title,
            at: at.clone(),
            private,
        };
        let _new = self
            .events
            .insert(&(owner.clone(), at.clone(), env::unique_id()), &event)?;
        env::emit(CalendarEvent::Added { owner, at }, Audience::All)?;
        Ok(())
    }

    /// The caller's own events in time order. Through another app (`env::via()`), the private
    /// ones are left out.
    #[app::view]
    #[app::allow(app = "*")]
    pub fn my_events(&self) -> app::Result<Vec<Event>> {
        let through_app = env::via().is_some();
        let mut events = Vec::new();
        for entry in self
            .events
            .prefix_iter(&[owner()?.to_key_part()], FindOpts::default())
        {
            let (_, event) = entry?;
            if !(through_app && event.private) {
                events.push(event);
            }
        }
        Ok(events)
    }
}
