//! The app environment: who is calling, org and team membership, time, randomness, events.
//!
//! In the guest every function reaches the host through the `calimero:app` imports. On the
//! host target (tests) they read a thread-local [`mock`] state instead.

use std::cell::Cell;
use std::io::Write;
use std::time::SystemTime;

use serde::Serialize;
use serde_json::json;

use crate::error::{AppError, Result};

#[cfg(target_arch = "wasm32")]
mod wasm;
#[cfg(target_arch = "wasm32")]
use wasm as backend;

#[cfg(not(target_arch = "wasm32"))]
pub mod mock;
#[cfg(not(target_arch = "wasm32"))]
use mock as backend;

/// Who invoked the method (D13).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Caller {
    /// A signed-in account.
    Account(String),
    /// An API key acting for an account.
    ApiKey {
        /// The key's id.
        key_id: String,
        /// The account the key acts for.
        account: String,
    },
    /// No account (a public method).
    Public,
    /// The platform itself (timers, migrations).
    System,
    /// Another app, by id (a delivered xcall).
    App(String),
}

/// An account's role in an org.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum OrgRole {
    /// Below member.
    Guest,
    /// A member.
    Member,
    /// An admin.
    Admin,
    /// The owner.
    Owner,
}

/// Who receives an event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Audience {
    /// Everyone subscribed to the app.
    All,
    /// The caller of the run.
    Caller,
    /// These accounts.
    Accounts(Vec<String>),
    /// The members of a team.
    Team(String),
    /// The members of the app's org.
    Org,
    /// The admins of the app's org.
    OrgAdmins,
}

/// A timer tick, in milliseconds since the epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimerInfo {
    /// The slot this tick is for.
    pub scheduled: u64,
    /// The previous slot applied.
    pub previous: u64,
}

thread_local! {
    static TIMER: Cell<Option<TimerInfo>> = const { Cell::new(None) };
}

/// The caller of this invocation.
pub fn caller() -> Caller {
    backend::caller()
}

/// This app's id.
pub fn app_id() -> String {
    backend::app_id()
}

/// The account that owns the app.
pub fn app_owner() -> String {
    backend::app_owner()
}

/// The org the app belongs to, if any.
pub fn org() -> Option<String> {
    backend::org()
}

/// `account`'s role in the app's org; none when the app has no org or they are not in it.
pub fn org_role(account: &str) -> Option<OrgRole> {
    backend::org_role(account)
}

/// Whether `account` is in `team`.
pub fn in_team(account: &str, team: &str) -> bool {
    backend::in_team(account, team)
}

/// The log index this run applies on top of.
pub fn applied_index() -> u64 {
    backend::applied_index()
}

/// A unique id: `<leader term>.<call seq>.<n>` (D52). Prefer it to `Counter::increment` (D47).
pub fn unique_id() -> String {
    backend::unique_id()
}

/// The wall clock in milliseconds since the epoch. Logged runs see the entry's timestamp.
pub fn time_now() -> u64 {
    backend::time_now_ms().unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
    })
}

/// `len` random bytes; seeded per logged run, so deterministic when replayed.
pub fn random_bytes(len: usize) -> Vec<u8> {
    backend::random_bytes(len)
}

/// Emit an event: `data` is serialized as JSON, like call arguments and results.
pub fn emit<T: Serialize + ?Sized>(name: &str, data: &T, audience: Audience) -> Result<()> {
    let data = serde_json::to_vec(data)
        .map_err(|e| AppError::with_fields("Emit", json!({ "message": e.to_string() })))?;
    backend::emit(name, data, audience)
}

/// The tick this timer method runs for; none in any other invocation.
pub fn timer() -> Option<TimerInfo> {
    TIMER.with(Cell::get)
}

/// Set the timer context. Called by the generated `handle` for `kind = timer`.
pub fn set_timer(info: Option<TimerInfo>) {
    TIMER.with(|t| t.set(info));
}

/// Write a line to the run's log (stderr; the host keeps 64 KiB per run, D50).
pub fn log(message: &str) {
    // The host owns the sink; a failed write to it has nowhere to be reported.
    let _ = writeln!(std::io::stderr(), "{message}");
}
