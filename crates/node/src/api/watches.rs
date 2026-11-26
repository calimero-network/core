//! Live views over HTTP (the registry is [`crate::watch`]).
//!
//! - `GET /apps/{app}/events?watch=true[&stream=<id>]` opens a watch stream (SSE). Its first
//!   frame names it: `event: stream`, `data: {"stream": "<id>"}`. A replica of the app
//!   serves it; any other node proxies it to one (`internal/events`), so the stream id names
//!   the node that holds it (`<node>.<random>`). `stream=<id>` with `Last-Event-ID:
//!   <index>:<seq>` resumes a stream within [`crate::watch::RESUME_GRACE`]: every watch
//!   touched after `index` is sent its current answer again; a stream that is gone is
//!   answered with a new one (its first frame names another id) followed by
//!   `event: watch-reset`, `data: {}`: the client registers its watches again.
//! - `POST /apps/{app}/watches { stream, view, args, intervalMs? }` runs the view as the
//!   caller through the gate `/views` uses, with the read-set recorder, and answers
//!   `{ sub, index, answer }`. The view runs again for the watch at most once per the
//!   interval its app sets (`watchIntervalMs` in the ABI, default 1000); a client's
//!   `intervalMs` can only lengthen it, up to 10000.
//! - `DELETE /apps/{app}/watches/{sub}` forgets one.
//! - The stream then carries `event: view-result`, `id: <index>:<seq>`,
//!   `data: {"sub", "index", "answer"}` (`"error"` in place of `"answer"` for an app
//!   error), only when the answer differs from the last one the client got;
//!   `event: view-changed`, `data: {"sub", "index"}` for an answer over
//!   [`MAX_PUSHED_ANSWER_BYTES`] (the client reads it); and `event: watch-ended`,
//!   `data: {"sub", "reason"}`. It carries no app events.

#[allow(
    clippy::wildcard_imports,
    reason = "the area modules read `api.rs`'s items, as the others do"
)]
use super::*;

use calimero_identity::{b64url, from_b64url, random};
use tokio::sync::{broadcast, mpsc};

use super::calls::not_reserved;
use super::internal::{caller_may_hold_app, peer};
use crate::app::Changed;
use crate::watch::{
    interval_of, Due, EndReason, Full, Holder, Notice, Watch, WatchStream, EXPIRY_CHECK,
    MAX_PUSHED_ANSWER_BYTES, MAX_WATCHES_PER_NODE, MAX_WATCHES_PER_STREAM, MAX_WATCH_ARGS_BYTES,
    STREAM_BUFFER,
};

/// Who holds a request's credential, for the stream and its watches.
pub(super) fn holder_of(p: &Principal) -> Holder {
    Holder {
        caller: p.caller.clone(),
        session: p.session.clone(),
        api_key: p.api_key.clone(),
        issuer_release: p.issuer_release.clone(),
    }
}

/// The principal a holder stands for, judged as a request's would be.
fn principal_of(holder: &Holder) -> Principal {
    Principal {
        caller: holder.caller.clone(),
        session: holder.session.clone(),
        api_key: holder.api_key.clone(),
        issuer_release: holder.issuer_release.clone(),
        consent_page: false,
        sys_index: None,
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
    let reset = resume.is_some() && kept.is_none();
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
            let system = node.system.subscribe_applied();
            let _task = tokio::spawn(drive(
                Arc::clone(node),
                Arc::clone(&stream),
                changes,
                system,
            ));
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
    let named = Event::default()
        .event("stream")
        .data(json!({ "stream": stream.id }).to_string());
    let first = futures_util::stream::iter(
        std::iter::once(named).chain(reset.then(|| frame(Notice::Reset))),
    );
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

/// One notice as its SSE frame.
fn frame(n: Notice) -> Event {
    match n {
        Notice::Result {
            sub,
            index,
            seq,
            answer,
        } => {
            let data = match answer {
                Ok(answer) => json!({ "sub": sub, "index": index, "answer": answer }),
                Err(error) => json!({ "sub": sub, "index": index, "error": error }),
            };
            Event::default()
                .id(format!("{index}:{seq}"))
                .event("view-result")
                .data(data.to_string())
        }
        Notice::Changed { sub, index, seq } => Event::default()
            .id(format!("{index}:{seq}"))
            .event("view-changed")
            .data(json!({ "sub": sub, "index": index }).to_string()),
        Notice::Ended { sub, reason } => Event::default()
            .event("watch-ended")
            .data(json!({ "sub": sub, "reason": reason.as_str() }).to_string()),
        Notice::Reset => Event::default().event("watch-reset").data("{}"),
    }
}

/// A stream's task: marks the watches each applied change touches (and, on each applied
/// system entry, judges the holder again and marks the views that read the directory), runs
/// what is due (the first change at once, then at most once per watch interval, or at once
/// when a client resumes), and forgets the stream once it has been without a client for
/// [`crate::watch::RESUME_GRACE`] or the app leaves this node.
async fn drive(
    node: Arc<Node>,
    stream: Arc<WatchStream>,
    mut changes: broadcast::Receiver<Arc<Changed>>,
    mut system: tokio::sync::watch::Receiver<u64>,
) {
    let mut next = None;
    loop {
        let check = tokio::time::Instant::now() + EXPIRY_CHECK;
        let wake_at = next
            .map(tokio::time::Instant::from_std)
            .map_or(check, |n: tokio::time::Instant| n.min(check));
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
            changed = system.changed() => {
                if changed.is_err() {
                    break;
                }
                system_changed(&node, &stream);
            }
            () = tokio::time::sleep_until(wake_at) => {
                if stream.expired() {
                    break;
                }
            }
            () = stream.wake.notified() => {}
        }
        next = flush(&node, &stream).await;
    }
    node.watches.close(&stream.id);
}

/// An applied system entry: a holder this replica's system state no longer admits loses
/// every watch at once (the next frame would judge it on fresh state anyway); the views that
/// read the directory, a blob's metadata or the clock are run again.
fn system_changed(node: &Node, stream: &WatchStream) {
    let principal = principal_of(&stream.holder);
    if !node.sys(|s| principal.admitted_on(s, &stream.app, now_secs())) {
        if let Some(out) = stream.out() {
            for sub in stream.subs() {
                end(node, stream, &out, sub, EndReason::AccessLost);
            }
        }
        return;
    }
    stream.mark_system(last_entry(node, &stream.app));
}

/// The app's last applied entry on this replica (0 if it is not hosted here).
fn last_entry(node: &Node, app_id: &str) -> u64 {
    node.hosted(app_id)
        .map_or(0, |g| g.read_local(|m, _| m.state.last_index))
}

/// Runs every watch that is due again as its holder and sends what changed, once the holder
/// is judged on fresh state. Answers when the next watch is due.
async fn flush(node: &Arc<Node>, stream: &Arc<WatchStream>) -> Option<std::time::Instant> {
    // Without a client the marks wait for one: a resumed stream sends them at once.
    let out = stream.out()?;
    let (due, _) = stream.take_due(std::time::Instant::now());
    for d in due {
        match access(node, stream, &d.view).await {
            Access::Unknown => stream.remark(&d.sub, d.index, false),
            Access::Lost => end(node, stream, &out, d.sub, EndReason::AccessLost),
            Access::Granted => refresh(node, stream, &out, d).await,
        }
    }
    stream.next_due()
}

enum Access {
    Granted,
    Lost,
    /// Could not be judged now (the bundle could not be read): try again next interval.
    Unknown,
}

/// Whether the stream's holder may still call `view`: their credential still admits them
/// to the app on the cluster's latest system state (as `/events` judges its streams: an
/// ended session, a removed passkey, a revoked or expired API key, a session bound to
/// another package), and the ABI still lets them call it.
async fn access(node: &Node, stream: &WatchStream, view: &str) -> Access {
    let principal = principal_of(&stream.holder);
    if !node.sys_caught_up().await {
        return Access::Unknown;
    }
    if !node.sys(|s| principal.admitted_on(s, &stream.app, now_secs())) {
        return Access::Lost;
    }
    if let Some(key) = &stream.holder.api_key {
        let scoped = key
            .methods
            .as_ref()
            .is_none_or(|m| m.iter().any(|x| x == view || x == "*"));
        if !scoped {
            return Access::Lost;
        }
    }
    let Some(app) = node.sys(|s| s.apps.get(&stream.app).cloned()) else {
        return Access::Lost;
    };
    match node
        .abi_gate(&app, view, &stream.holder.caller, true, false)
        .await
    {
        Ok(()) => Access::Granted,
        Err(e) if refuses(&e) => Access::Lost,
        Err(_) => Access::Unknown,
    }
}

/// An answer that says the caller may not call the view (as opposed to "not now").
fn refuses(e: &ApiError) -> bool {
    matches!(e.status, 401 | 403 | 404)
}

/// Runs the watch's view again as the holder, keeps what it read, and sends its answer if
/// it differs from the one the client holds. A view that can no longer be watched (refused,
/// or reading past the read-set caps) ends the watch; a trap or timeout tries again after
/// the interval.
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
        via: None,
    };
    let run = match node.view_component(&g, &req, None, true).await {
        Ok(run) => run,
        Err(e) if refuses(&e) => return end(node, stream, out, d.sub, EndReason::ViewRefused),
        Err(_) => return stream.remark(&d.sub, d.index, false),
    };
    let answer = match run.answer.status {
        Status::Ok => Ok(run.answer.result),
        Status::AppError => Err(run.answer.error),
        _ => return stream.remark(&d.sub, d.index, false),
    };
    let Some(set) = run.read_set else {
        return stream.remark(&d.sub, d.index, false);
    };
    if set.unbounded().is_some() {
        return end(node, stream, out, d.sub, EndReason::ReadSetUnbounded);
    }
    stream.refresh(&d.sub, set, run.last_entry, || {
        g.read_local(|m, _| m.state.last_index)
    });
    if !stream.answer_changed(&d.sub, &answer) {
        return;
    }
    let (index, seq) = (run.last_entry.max(d.index), stream.next_seq());
    let bytes = match &answer {
        Ok(v) | Err(v) => serde_json::to_vec(v).map_or(usize::MAX, |b| b.len()),
    };
    let notice = if bytes > MAX_PUSHED_ANSWER_BYTES {
        Notice::Changed {
            sub: d.sub.clone(),
            index,
            seq,
        }
    } else {
        Notice::Result {
            sub: d.sub.clone(),
            index,
            seq,
            answer,
        }
    };
    if out.try_send(notice).is_err() {
        stream.remark(&d.sub, index, true);
    }
}

/// Forgets a watch and tells the client (if it is still there to hear it).
fn end(
    node: &Node,
    stream: &WatchStream,
    out: &mpsc::Sender<Notice>,
    sub: String,
    reason: EndReason,
) {
    if node.watches.remove(stream, &sub) {
        let _ = out.try_send(Notice::Ended { sub, reason });
    }
}

/// What `POST /apps/{app}/watches` answers.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WatchAdded {
    sub: String,
    /// The app's last entry in the state the view read: a stream's `Last-Event-ID` index.
    index: u64,
    /// The view's answer now.
    answer: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct WatchBody {
    stream: String,
    view: String,
    #[serde(default)]
    args: Value,
    /// A longer interval than the app's for this watch, in ms; a shorter one is ignored.
    #[serde(default)]
    interval_ms: Option<u64>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    interval_ms: Option<u64>,
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
        interval_ms: body.interval_ms,
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
        via: None,
    };
    let run = node.view_component(&g, &req, None, true).await?;
    let a = &run.answer;
    if let Some(e) = super::calls::component_failure(a.index, &a.status, &a.error, a.timeout) {
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
    let answer = run.answer.result;
    let watch = Watch::new(
        add.view,
        add.args,
        set,
        run.last_entry,
        interval_of(method.watch_interval, add.interval_ms),
        &Ok(answer.clone()),
    );
    node.watches
        .add(&stream, sub.clone(), watch, || {
            g.read_local(|m, _| m.state.last_index)
        })
        .map_err(too_many)?;
    serde_json::to_value(WatchAdded {
        sub,
        index: run.last_entry,
        answer,
    })
    .map_err(|e| ApiError::internal(&e.to_string()))
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

/// A watch stream: served here when this node hosts the app, else proxied to a replica
/// (the one that holds the stream being resumed, if it is one).
pub(super) async fn watch_events(
    node: &Arc<Node>,
    app: &crate::types::AppRecord,
    p: &Principal,
    resume: Option<&str>,
    last: Option<u64>,
    slot: impl Send + Sync + 'static,
) -> Result<Response, ApiError> {
    let holder = holder_of(p);
    // An id this cluster made (`<node>.<base64url>`); anything else resumes nothing.
    let resume = resume.filter(|id| {
        owner_of(id).is_some()
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    });
    if let Some(stream) = local_stream(node, &app.app_id, holder.clone(), resume, last)? {
        let stream = stream.map(move |e| {
            let _held = &slot;
            e
        });
        return Ok(Sse::new(stream)
            .keep_alive(KeepAlive::default())
            .into_response());
    }
    let owner = resume
        .and_then(owner_of)
        .filter(|id| app.replicas.contains(id));
    let addr = node
        .sys(|s| {
            owner
                .iter()
                .chain(app.replicas.iter())
                .find_map(|r| s.nodes.get(r).map(|n| n.addr.clone()))
        })
        .ok_or_else(|| ApiError::unavailable("no replica known"))?;
    let mut path = format!(
        "internal/events?app={}&watch=true&holder={}",
        app.app_id,
        holder_param(&holder)
    );
    if let Some(id) = resume {
        path.push_str(&format!("&stream={id}"));
    }
    if let Some(last) = last {
        path.push_str(&format!("&last={last}"));
    }
    let resp = node
        .peer_stream(&addr, &path)
        .await
        .map_err(|e| ApiError::unavailable(&e.to_string()))?;
    let body = axum::body::Body::from_stream(resp.bytes_stream().map(move |b| {
        let _held = &slot;
        b
    }));
    Ok((
        [
            ("content-type", "text/event-stream"),
            ("cache-control", "no-store"),
        ],
        body,
    )
        .into_response())
}

/// This area's routes, merged into [`super::router`].
pub(super) fn routes() -> Router<Arc<Node>> {
    Router::new()
        .route("/apps/{app}/watches", post(watch_add))
        .route("/apps/{app}/watches/{sub}", delete(watch_remove))
        .route("/internal/watches", post(internal_watch_add))
        .route("/internal/watches/remove", post(internal_watch_remove))
}
