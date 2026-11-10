//! View subscriptions over HTTP (PoC; the registry is [`crate::watch`]).
//!
//! - `GET /apps/{app}/events?watch=true[&stream=<id>]` opens a watch stream (SSE). Its first
//!   frame names it: `event: stream`, `data: {"stream": "<id>"}`. A replica of the app
//!   serves it; any other node proxies it to one (`internal/events`), so the stream id names
//!   the node that holds it (`<node>.<random>`). `stream=<id>` with `Last-Event-ID:
//!   <index>:<seq>` resumes a stream within [`crate::watch::RESUME_GRACE`]: every watch touched after
//!   `index` is noticed again; a stream that is gone is answered with a new one (its first
//!   frame names another id), which tells the client to register its watches again and
//!   re-read.
//! - `POST /apps/{app}/watches { stream, view, args }` runs the view as the caller through
//!   the gate `/views` uses, with the read-set recorder, and answers `{ sub, index }`.
//! - `DELETE /apps/{app}/watches/{sub}` forgets one.
//! - The stream then carries `event: view-changed`, `id: <index>:<seq>`,
//!   `data: {"sub", "index"}` (nothing else, ever) and `event: watch-ended`,
//!   `data: {"sub"}`. It carries no app events.

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::sse::Event;
use axum::{Extension, Json};
use calimero_identity::{b64url, from_b64url, random};
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc};

use super::{caller_may_hold_app, gate, not_reserved, peer, typed_id, ApiResult, Principal};
use crate::app::{Changed, Status};
use crate::error::ApiError;
use crate::node::{Node, ViewRequest};
use crate::types::{now_secs, Caller};
use crate::watch::{
    Due, Full, Holder, Notice, Watch, WatchStream, MAX_WATCHES_PER_NODE, MAX_WATCHES_PER_STREAM,
    MAX_WATCH_ARGS_BYTES, NOTICE_INTERVAL, STREAM_BUFFER,
};

/// Who holds a request's credential, for the stream and its watches.
pub(super) fn holder_of(p: &Principal) -> Holder {
    Holder {
        caller: p.caller.clone(),
        session: p.session.as_ref().map(|s| s.session.clone()),
        passkey: p.session.as_ref().and_then(|s| s.passkey.clone()),
    }
}

/// The holder as a peer's stream query carries it (base64url JSON).
pub(super) fn holder_param(holder: &Holder) -> String {
    b64url(&serde_json::to_vec(holder).unwrap_or_default())
}

pub(super) fn holder_from_param(param: &str) -> Result<Holder, ApiError> {
    from_b64url(param)
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .ok_or_else(|| ApiError::bad_request("BAD_HOLDER", "a watch stream names its holder"))
}

/// The index of a `Last-Event-ID: <index>:<seq>` header.
pub(super) fn last_event_index(headers: &HeaderMap) -> Option<u64> {
    let id = headers.get("last-event-id")?.to_str().ok()?;
    id.split(':').next()?.parse().ok()
}

/// The node an id made here names (`<node>.<random>`).
pub(super) fn owner_of(id: &str) -> Option<u64> {
    id.split_once('.')?.0.parse().ok()
}

/// Clears the stream's connection when its SSE body is dropped (the client went).
struct Attached {
    stream: Arc<WatchStream>,
    out: mpsc::Sender<Notice>,
}

impl Drop for Attached {
    fn drop(&mut self) {
        self.stream.detach(&self.out);
    }
}

/// A watch stream served here: the one named `resume` if this node still holds it for
/// `holder` in `app`, else a new one. `None` if this node does not host the app.
pub(super) fn local_stream(
    node: &Arc<Node>,
    app_id: &str,
    holder: Holder,
    resume: Option<&str>,
    last: Option<u64>,
) -> Result<Option<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let Some(g) = node.hosted(app_id) else {
        return Ok(None);
    };
    let kept = resume
        .and_then(|id| node.watches.stream(id))
        .filter(|s| s.app == app_id && s.holder.caller == holder.caller);
    let stream = match kept {
        Some(stream) => stream,
        None => {
            let id = format!("{}.{}", node.config.node_id, b64url(&random::<16>()));
            let stream = node
                .watches
                .open(id, app_id.to_owned(), holder)
                .ok_or_else(|| {
                    ApiError::new(
                        429,
                        "RATE_LIMITED",
                        "this node holds as many watch streams as it can",
                    )
                })?;
            let changes = g.read_local(|m, _| m.changes_tx.subscribe());
            let _task = tokio::spawn(drive(Arc::clone(node), Arc::clone(&stream), changes));
            stream
        }
    };
    let (tx, rx) = mpsc::channel(STREAM_BUFFER);
    // A new stream has nothing to resume: `last` names another stream's notices.
    stream.attach(tx.clone(), kept_last(&stream, resume, last));
    let attached = Attached {
        stream: Arc::clone(&stream),
        out: tx,
    };
    let first = futures_util::stream::once(async move {
        Event::default()
            .event("stream")
            .data(json!({ "stream": stream.id }).to_string())
    });
    let notices = tokio_stream::wrappers::ReceiverStream::new(rx).map(frame);
    Ok(Some(first.chain(notices).map(move |e| {
        let _held = &attached;
        Ok(e)
    })))
}

/// `last` only for the stream it was resumed by.
fn kept_last(stream: &WatchStream, resume: Option<&str>, last: Option<u64>) -> Option<u64> {
    (resume == Some(stream.id.as_str()))
        .then_some(last)
        .flatten()
}

/// One notice as its SSE frame: nothing but the watch id and the entry index.
fn frame(n: Notice) -> Event {
    match n {
        Notice::Changed { sub, index, seq } => Event::default()
            .id(format!("{index}:{seq}"))
            .event("view-changed")
            .data(json!({ "sub": sub, "index": index }).to_string()),
        Notice::Ended { sub } => Event::default()
            .event("watch-ended")
            .data(json!({ "sub": sub }).to_string()),
    }
}

/// A stream's task: marks the watches each applied change touches, sends what is due once
/// per [`NOTICE_INTERVAL`] (or at once when a client resumes), and forgets the stream once
/// it has been without a client for [`crate::watch::RESUME_GRACE`] or the app leaves this node.
async fn drive(
    node: Arc<Node>,
    stream: Arc<WatchStream>,
    mut changes: broadcast::Receiver<Arc<Changed>>,
) {
    let mut tick = tokio::time::interval(NOTICE_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            change = changes.recv() => match change {
                Ok(change) => stream.mark(&change),
                // Fallen behind: anything may have changed.
                Err(broadcast::error::RecvError::Lagged(_)) => stream.mark(&Changed {
                    index: last_entry(&node, &stream.app),
                    keys: None,
                }),
                Err(broadcast::error::RecvError::Closed) => break,
            },
            _ = tick.tick() => {
                if stream.expired() {
                    break;
                }
                flush(&node, &stream).await;
            }
            () = stream.wake.notified() => flush(&node, &stream).await,
        }
    }
    node.watches.close(&stream.id);
}

/// The app's last applied entry on this replica (0 if it is not hosted here).
fn last_entry(node: &Node, app_id: &str) -> u64 {
    node.hosted(app_id)
        .map_or(0, |g| g.read_local(|m, _| m.state.last_index))
}

/// Sends a notice for every watch due one whose holder may still call its view, then runs
/// the view again as the holder to learn what it reads now.
async fn flush(node: &Arc<Node>, stream: &Arc<WatchStream>) {
    let Some(out) = stream.out() else {
        return;
    };
    let due = stream.take_due();
    if due.is_empty() {
        return;
    }
    // Access is judged on the cluster's latest system state; unknown, nothing is sent.
    if !node.sys_fresh().await {
        for d in &due {
            stream.remark(&d.sub, d.index);
        }
        return;
    }
    for d in due {
        match access(node, stream, &d.view).await {
            Access::Unknown => {
                stream.remark(&d.sub, d.index);
                continue;
            }
            Access::Lost => {
                end(node, stream, &out, d.sub);
                continue;
            }
            Access::Granted => {}
        }
        let notice = Notice::Changed {
            sub: d.sub.clone(),
            index: d.index,
            seq: stream.next_seq(),
        };
        if out.try_send(notice).is_err() {
            stream.remark(&d.sub, d.index);
            continue;
        }
        refresh(node, stream, &out, d).await;
    }
}

enum Access {
    Granted,
    Lost,
    /// Could not be judged now (the bundle could not be read): try again next interval.
    Unknown,
}

/// Whether the stream's holder may still call `view`: the app still reaches them, their
/// session or API key is still good (and still scoped to the app and the view), and the ABI
/// still lets them call it.
async fn access(node: &Node, stream: &WatchStream, view: &str) -> Access {
    let holder = &stream.holder;
    let account = holder.caller.account();
    let now = now_secs();
    let standing = node.sys(|s| {
        let Some(app) = s.apps.get(&stream.app) else {
            return false;
        };
        if !s.may_reach(app, account) {
            return false;
        }
        if holder
            .session
            .as_ref()
            .is_some_and(|session| s.revoked_sessions.contains(session))
        {
            return false;
        }
        if let (Some(passkey), Some(account)) = (&holder.passkey, account) {
            if s.credentials.get(passkey).map(String::as_str) != Some(account) {
                return false;
            }
        }
        if let Caller::ApiKey { key, .. } = &holder.caller {
            let Some(rec) = s.api_keys.values().find(|r| &r.key_id == key) else {
                return false;
            };
            let scoped_app = rec.apps.as_ref().is_none_or(|a| a.contains(&stream.app));
            let scoped_view = rec
                .methods
                .as_ref()
                .is_none_or(|m| m.iter().any(|x| x == view || x == "*"));
            if rec.expires_at <= now || !scoped_app || !scoped_view {
                return false;
            }
        }
        true
    });
    if !standing {
        return Access::Lost;
    }
    let Some(app) = node.sys(|s| s.apps.get(&stream.app).cloned()) else {
        return Access::Lost;
    };
    match node.abi_gate(&app, view, &holder.caller, true, false).await {
        Ok(()) => Access::Granted,
        Err(e) if refuses(&e) => Access::Lost,
        Err(_) => Access::Unknown,
    }
}

/// An answer that says the caller may not call the view (as opposed to "not now").
fn refuses(e: &ApiError) -> bool {
    matches!(e.status, 401 | 403 | 404)
}

/// Runs the watch's view again as the holder and keeps what it read. A view that can no
/// longer be watched (refused, or reading past the read-set caps) ends the watch; a trap or
/// timeout keeps the previous read set.
async fn refresh(node: &Arc<Node>, stream: &WatchStream, out: &mpsc::Sender<Notice>, d: Due) {
    let Some(g) = node.hosted(&stream.app) else {
        return;
    };
    let req = ViewRequest {
        app_id: stream.app.clone(),
        method: d.view,
        args: d.args,
        caller: stream.holder.caller.clone(),
        strong: false,
        min_index: 0,
    };
    match node.view_component(&g, &req, true).await {
        Ok(run) => match run.read_set {
            Some(set) if set.unbounded().is_some() => end(node, stream, out, d.sub),
            Some(set) if matches!(run.answer.status, Status::Ok | Status::AppError) => {
                stream.refresh(&d.sub, set, run.last_entry, || {
                    g.read_local(|m, _| m.state.last_index)
                });
            }
            _ => {}
        },
        Err(e) if refuses(&e) => end(node, stream, out, d.sub),
        Err(_) => {}
    }
}

/// Forgets a watch and tells the client (if it is still there to hear it).
fn end(node: &Node, stream: &WatchStream, out: &mpsc::Sender<Notice>, sub: String) {
    if node.watches.remove(stream, &sub) {
        let _ = out.try_send(Notice::Ended { sub });
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct WatchBody {
    stream: String,
    view: String,
    #[serde(default)]
    args: Value,
}

/// A watch to add on the node that holds its stream (`internal/watches`).
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AddWatch {
    app: String,
    stream: String,
    view: String,
    args: Value,
    holder: Holder,
}

/// A watch to remove there (`internal/watches/remove`).
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RemoveWatch {
    app: String,
    sub: String,
    holder: Holder,
}

/// `POST /apps/{app}/watches`.
pub(super) async fn watch_add(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<WatchBody>,
) -> ApiResult {
    not_reserved(&body.view)?;
    let args_len = serde_json::to_vec(&body.args).map_or(usize::MAX, |a| a.len());
    if args_len > MAX_WATCH_ARGS_BYTES {
        return Err(ApiError::new(
            413,
            "ARGS_TOO_LARGE",
            &format!("a watched view's arguments are at most {MAX_WATCH_ARGS_BYTES} bytes"),
        ));
    }
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    // The gate `/views` uses: who reaches the app, and an API key's app and method scope.
    let p = gate(&node, &headers, &app, &body.view, false).await?;
    let add = AddWatch {
        app: app_id,
        stream: body.stream,
        view: body.view,
        args: body.args,
        holder: holder_of(&p),
    };
    let owner = owner_of(&add.stream).ok_or_else(no_stream)?;
    if owner == node.config.node_id {
        return Ok(Json(add_local(&node, add).await?));
    }
    forward(&node, owner, "internal/watches", &add).await
}

/// `DELETE /apps/{app}/watches/{sub}`.
pub(super) async fn watch_remove(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, sub)): Path<(String, String)>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let p = gate(&node, &headers, &app, "*", false).await?;
    let remove = RemoveWatch {
        app: app_id,
        sub,
        holder: holder_of(&p),
    };
    let owner = owner_of(&remove.sub).ok_or_else(no_watch)?;
    if owner == node.config.node_id {
        return Ok(Json(remove_local(&node, &remove)?));
    }
    forward(&node, owner, "internal/watches/remove", &remove).await
}

/// `POST /internal/watches`: a peer passes on a watch the caller registered there.
pub(super) async fn internal_watch_add(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    key: Option<Extension<calimero_sealed::PeerKey>>,
    Json(add): Json<AddWatch>,
) -> ApiResult {
    let caller = peer(&node, &headers, key).await?;
    caller_may_hold_app(&node, &caller, &add.app).await?;
    Ok(Json(add_local(&node, add).await?))
}

/// `POST /internal/watches/remove`.
pub(super) async fn internal_watch_remove(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    key: Option<Extension<calimero_sealed::PeerKey>>,
    Json(remove): Json<RemoveWatch>,
) -> ApiResult {
    let caller = peer(&node, &headers, key).await?;
    caller_may_hold_app(&node, &caller, &remove.app).await?;
    Ok(Json(remove_local(&node, &remove)?))
}

/// Passes a watch request to the node that holds the stream, answering what it answers.
async fn forward(node: &Node, owner: u64, path: &str, body: &impl Serialize) -> ApiResult {
    let addr = node.replica_addr(owner).ok_or_else(no_stream)?;
    let body = serde_json::to_value(body).map_err(|e| ApiError::internal(&e.to_string()))?;
    node.peer_forward(&addr, path, &body)
        .await
        .map(Json)
        .map_err(|e| match e.downcast::<ApiError>() {
            Ok(e) => e,
            Err(e) => ApiError::unavailable(&e.to_string()),
        })
}

fn no_stream() -> ApiError {
    ApiError::new(404, "UNKNOWN_STREAM", "no such watch stream")
}

fn no_watch() -> ApiError {
    ApiError::new(404, "UNKNOWN_WATCH", "no such watch")
}

/// Adds a watch to a stream this node holds: the view runs as the holder (its ABI gate
/// included) with the read-set recorder, and only a subscribable view whose reads stay
/// within the caps is kept.
async fn add_local(node: &Arc<Node>, add: AddWatch) -> Result<Value, ApiError> {
    let stream = node
        .watches
        .stream(&add.stream)
        .filter(|s| s.app == add.app && s.holder.caller == add.holder.caller)
        .ok_or_else(no_stream)?;
    let g = node
        .hosted(&add.app)
        .ok_or_else(|| ApiError::unavailable("the app is no longer hosted here"))?;
    let app = node.app(&add.app).await?;
    let prepared = node.prepared_for(&node.running_bundle(&app)).await?;
    let Some(method) = prepared.abi.methods.get(&add.view) else {
        return Err(ApiError::not_found("no such view"));
    };
    if method.kind != calimero_app_host::MethodKind::View {
        return Err(ApiError::bad_request(
            "NOT_A_VIEW",
            &format!("{} is not a view", add.view),
        ));
    }
    if !method.subscribable {
        return Err(ApiError::bad_request(
            "NOT_SUBSCRIBABLE",
            &format!("{} is not a subscribable view", add.view),
        ));
    }
    if stream.len() >= MAX_WATCHES_PER_STREAM || node.watches.total() >= MAX_WATCHES_PER_NODE {
        return Err(too_many(Full::Stream));
    }
    let req = ViewRequest {
        app_id: add.app.clone(),
        method: add.view.clone(),
        args: add.args.clone(),
        caller: add.holder.caller.clone(),
        strong: false,
        min_index: 0,
    };
    let run = node.view_component(&g, &req, true).await?;
    let a = &run.answer;
    if let Some(e) = super::component_failure(a.index, &a.status, &a.error, a.timeout) {
        return Err(e);
    }
    let set = run
        .read_set
        .ok_or_else(|| ApiError::internal("the view's reads were not recorded"))?;
    if let Some(why) = set.unbounded() {
        return Err(ApiError::new(
            422,
            "READ_SET_UNBOUNDED",
            &format!("{} cannot be watched: {why}", add.view),
        ));
    }
    let sub = format!("{}.{}", node.config.node_id, typed_id::<12>());
    node.watches
        .add(
            &stream,
            sub.clone(),
            Watch::new(add.view, add.args, set, run.last_entry),
            || g.read_local(|m, _| m.state.last_index),
        )
        .map_err(too_many)?;
    Ok(json!({ "sub": sub, "index": run.answer.index }))
}

fn too_many(full: Full) -> ApiError {
    let what = match full {
        Full::Stream => format!("a stream holds at most {MAX_WATCHES_PER_STREAM} watches"),
        Full::Node => format!("this node holds at most {MAX_WATCHES_PER_NODE} watches"),
    };
    ApiError::new(429, "TOO_MANY_WATCHES", &what)
}

fn remove_local(node: &Node, remove: &RemoveWatch) -> Result<Value, ApiError> {
    let stream = node
        .watches
        .stream_of(&remove.sub)
        .filter(|s| s.app == remove.app && s.holder.caller == remove.holder.caller)
        .ok_or_else(no_watch)?;
    if !node.watches.remove(&stream, &remove.sub) {
        return Err(no_watch());
    }
    Ok(json!({}))
}
