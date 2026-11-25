//! The app environment: who is calling, org and team membership, time, randomness, events.
//!
//! In the guest every function reaches the host through the `calimero:app` imports. On the
//! host target (tests) they read a thread-local [`mock`] state instead.
//!
//! Ids in these comments cite `docs/specs/app-runtime-v0.14.md` (D decisions in section 2, R
//! requirements in section 8): D13 (every invocation has an explicit caller), D17 (actions are
//! durable, logged when started and when done), D20 (64-bit integers travel as decimal strings
//! in JSON), D25 (xcalls are deduplicated by a per-source watermark), D47 (`storage.add` and
//! `unique-id` avoid serializing calls), D50 (per-run limits on events, keys, cursors and
//! logs), D52 (unique ids are `<leader term>.<call seq>.<n>`), D76 (calls, timers and
//! deliveries may send outbound HTTP, recorded in the entry), R8 (the blob API and its
//! lifecycle), R14 (secrets for outbound calls).

use std::cell::Cell;
use std::io::Write;
use std::time::SystemTime;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::json;

use crate::app::AppEvent;
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
    /// An anonymous visitor through one of the instance's public links: the link (its key)
    /// and the visitor's own Ed25519 public key (hex), which it proved it holds. It reaches
    /// only `#[app::public]` methods, has no account and no org role: `require_member` and
    /// the other guards refuse it with `LoginRequired`. Two visitors have different keys; the
    /// same browser keeps its key. WIT 0.5.6 (`context.visitor`).
    Visitor {
        /// The public link it came through.
        link: String,
        /// The visitor's Ed25519 public key, 64 lowercase hex.
        key: String,
    },
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
    /// The previous slot applied; none on a timer's first tick.
    pub previous: Option<u64>,
}

thread_local! {
    static TIMER: Cell<Option<TimerInfo>> = const { Cell::new(None) };
}

/// The caller of this invocation.
pub fn caller() -> Caller {
    backend::caller()
}

/// The app this call came through, when another app called as the person it serves:
/// `caller()` is then that person (an account or an API key), who granted the app this method
/// (`PUT /apps/{app}/delegations/{target}`), and the app's own rules for the person apply.
/// `None` for every other run, including another app's call as itself (`Caller::App`).
///
/// Use it when a call through an app should get less than the person's own call, e.g.
/// read-only through a plugin.
pub fn via() -> Option<String> {
    backend::via()
}

/// This app's id.
pub fn app_id() -> String {
    backend::app_id()
}

/// The app this instance's release references under `name` (`uses` in the release's
/// `Cargo.toml` metadata): the member the node created with this instance, or the app its
/// owner attached. `None` when nothing is linked under that name. Read it whenever you call
/// the app: an owner may attach another one later.
pub fn linked(name: &str) -> Option<String> {
    backend::linked(name)
}

/// The app whose unit this one is a member of: the instance whose release created this app
/// with it (`uses` with `mode = "create"`). `None` for an app no instance created, and for
/// one whose instance was deleted.
///
/// An app that serves others (a confidential KV store, a private inference endpoint) treats
/// `app:<unit_of()>` as its admin, so the instance that created it uses it with no grant step:
///
/// ```ignore
/// let admin = env::unit_of().is_some_and(|parent| env::caller() == Caller::App(parent));
/// ```
pub fn unit_of() -> Option<String> {
    backend::unit_of()
}

/// The scope of the caller's grant on this instance, as JSON text: what the instance's
/// managers set on the person's grant (or on the invite that made it), in the app's own
/// terms, such as `{"channel":"partners"}`. The platform admits people to the instance and
/// carries this for the app, which decides what it means: a chat keeps a partner invited with
/// a channel's scope to that channel.
///
/// `None` when the caller is no person (an app, a visitor, the system), is named on the
/// instance without a scope, or reaches it another way (as a member of its organization or a
/// team, or anyone): the app's whole. WIT 0.5.6 (`context.grant-scope`).
///
/// ```ignore
/// if let Some(scope) = env::grant_scope() {
///     let scope: Scope = serde_json::from_str(&scope)?;
///     require(scope.channel == channel)?;
/// }
/// ```
pub fn grant_scope() -> Option<String> {
    backend::grant_scope()
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

/// Emit an `#[app::event]` value: its variant name and JSON data (D20: 64-bit integers as
/// decimal strings).
///
/// # Errors
///
/// `Emit` with the host's reason (`calimero-app-host`'s `EMIT_*`); `RunLimit` when the
/// event would take the run past its 1 MiB of writes.
pub fn emit<E: AppEvent>(event: E, audience: Audience) -> Result<()> {
    let (name, data) = event.encode()?;
    emit_raw(name, &data, audience)
}

/// Emit an event by name: `data` is serialized as JSON, like call arguments and results.
pub fn emit_raw<T: Serialize + ?Sized>(name: &str, data: &T, audience: Audience) -> Result<()> {
    let data = serde_json::to_vec(data)
        .map_err(|e| AppError::with_fields("Emit", json!({ "message": e.to_string() })))?;
    backend::emit(name, data, audience)
}

/// Send `args` (serialized as JSON) to `method` of app `app` (D25): the message waits in
/// this run's outbox and the platform delivers it after the entry commits, only if the run
/// succeeds, at least once and in order per sending app, as `Caller::App(<this app>)`. The
/// target app must be in the same organization (or, for a personal app, have the same
/// owner) and its method must `allow(app = "<this app>")` or `allow(app = "*")`. Calls,
/// timers, `init` and deliveries only.
///
/// # Errors
///
/// `Xcall` with the host's reason: more than 100 xcalls in one run, an empty or overlong
/// app id or method, or a reserved (`__`) method; `RunLimit` when the run would pass its
/// 1 MiB of writes.
pub fn xcall<T: Serialize + ?Sized>(app: &str, method: &str, args: &T) -> Result<()> {
    let args = serde_json::to_vec(args)
        .map_err(|e| AppError::with_fields("Xcall", json!({ "message": e.to_string() })))?;
    backend::xcall(app, method, &args).map_err(|message| AppError::host_soft("Xcall", message))
}

/// Call `method` of app `app` synchronously and decode its JSON result: the target runs it
/// as one call in its own log, as `Caller::App(<this app>)`, before this run continues.
/// The target must be in the same organization (or, for a personal app, have the same
/// owner), a component app other than this one, and its method must
/// `allow(app = "<this app>")` or `allow(app = "*")`. Calls, timers and actions only; a
/// delivered xcall makes none.
///
/// It is a future: `.await` it in an `async fn` app method. Awaited one after another,
/// xcalls run one after another; joined (`app::join!`, `xcall::join_all`), the node runs
/// them at once, at most 16 per run in all (`MAX_SYNC_XCALLS`). For a target known at build
/// time, prefer the typed client a release's `uses` generates (`app::uses!()`).
///
/// Not atomic: the target's call commits whatever this run does next, so a target method
/// should be safe to have run (a captcha's `verify` spends the challenge either way). A
/// re-run of this call (a client's retry, a leader change) replays the target's answer:
/// the target's key is derived from this run's, the target, the method, the arguments
/// and how many identical calls came before in the run.
///
/// # Errors
///
/// `XcallFailed` with the fields of the failure: the target's app error (`code`,
/// `fields`), a refusal or a timeout (`message`); `BadOutput` when the result is not a `T`;
/// `BadArgs` when `args` do not serialize.
pub fn xcall_call<T: DeserializeOwned, A: Serialize + ?Sized>(
    app: &str,
    method: &str,
    args: &A,
) -> crate::xcall::Pending<Result<T>> {
    crate::xcall::Pending::call(app, method, args, crate::xcall::decode::<T>)
}

/// Run view `method` of app `app` on its latest committed state and decode its JSON
/// result, with the checks of [`xcall_call`]. Calls, timers, views and actions.
///
/// # Errors
///
/// As [`xcall_call`].
pub fn xcall_view<T: DeserializeOwned, A: Serialize + ?Sized>(
    app: &str,
    method: &str,
    args: &A,
) -> crate::xcall::Pending<Result<T>> {
    crate::xcall::Pending::view(app, method, args, crate::xcall::decode::<T>)
}

/// Send `requests` to the node as one batch (WIT `xcall.batch`): it runs them at once and
/// answers each in order, with the checks of [`xcall_call`] and [`xcall_view`] per request.
/// The executor behind `.await` calls it; call it yourself only to batch by hand.
pub fn xcall_batch(requests: &[crate::xcall::Request]) -> Vec<crate::xcall::Answer> {
    if requests.is_empty() {
        return Vec::new();
    }
    backend::xcall_batch(requests)
        .into_iter()
        .map(|answer| answer.map_err(|message| xcall_failure(&message)))
        .collect()
}

/// A synchronous xcall's failure: `XcallFailed` with the failure's JSON fields (a message
/// that is not a JSON object becomes `{"message": ..}`).
fn xcall_failure(message: &str) -> AppError {
    let fields = match serde_json::from_str::<serde_json::Value>(message) {
        Ok(v @ serde_json::Value::Object(_)) => v,
        _ => json!({ "message": message }),
    };
    AppError::with_fields("XcallFailed", fields)
}

/// Send `request` over `wasi:http` from a call, a timer or a delivery (D76), as an action's
/// `ctx.fetch` does: only to a URL under a prefix the app's signed manifest declares and its
/// organization approved, with the credentials its manifest binds to the host (`secrets`)
/// attached by the node, never by the app (R14; a request writing the retired
/// `{{secret:NAME}}`, or setting a bound header or field itself, is refused), at most 8
/// requests per run (`MAX_HTTP_REQUESTS`) and within the call's time budget. The exchange is recorded in the call's log
/// entry: followers and a retry with the same idempotency key read the recorded answer and
/// never fetch again. Views, `init` and migrations cannot send (the run traps).
///
/// # Errors
///
/// `Fetch` with the reason it was not sent or failed (refused by the gate, timed out,
/// unreachable): the app decides what that means.
pub fn fetch(request: &crate::action::Request) -> Result<crate::action::Response> {
    backend::fetch(request)
        .map_err(|message| AppError::with_fields("Fetch", json!({ "message": message })))
}

/// Start the `#[app::action]` `method` with `args` (D17): logged as started with this run's
/// entry, only if the run succeeds, then run by the leader outside the log with this run's
/// caller. Calls and timers only. Returns the action's key.
///
/// # Errors
///
/// `StartAction` with the host's reason (more than 100 in one run, a reserved name);
/// `RunLimit` when the run would pass its 1 MiB of writes.
pub fn start_action<T: Serialize + ?Sized>(method: &str, args: &T) -> Result<String> {
    let args = serde_json::to_vec(args)
        .map_err(|e| AppError::with_fields("StartAction", json!({ "message": e.to_string() })))?;
    backend::start_action(method, &args)
        .map_err(|message| AppError::host_soft("StartAction", message))
}

pub(crate) fn backend_action_key() -> String {
    backend::action_key()
}

pub(crate) fn backend_action_view(
    method: &str,
    args: &[u8],
) -> std::result::Result<Vec<u8>, String> {
    backend::action_view(method, args)
}

pub(crate) fn backend_action_call(
    method: &str,
    args: &[u8],
) -> std::result::Result<Vec<u8>, String> {
    backend::action_call(method, args)
}

pub(crate) fn backend_fetch(
    request: &crate::action::Request,
) -> std::result::Result<crate::action::Response, String> {
    backend::fetch(request)
}

/// Blob `id`'s metadata, if the app holds it (R8). Prefer [`crate::BlobRef::info`].
pub fn blob_info(id: &str) -> Option<crate::BlobInfo> {
    backend::blob_info(id)
}

/// A download grant for blob `id`, valid `ttl_seconds` from the run's clock. Prefer
/// [`crate::BlobRef::grant`].
pub fn blob_grant(id: &str, ttl_seconds: u32) -> String {
    backend::blob_grant(id, ttl_seconds)
}

/// Query the host's full-text index of `table` (views only). Prefer
/// [`crate::Table::search`], which reads the records back.
pub fn search_query(
    table: &str,
    text: &str,
    mode: crate::SearchMode,
    cursor: u32,
    limit: u32,
) -> std::result::Result<crate::search::RawPage, String> {
    backend::search_query(table, text, mode, cursor, limit)
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
