//! The API's routes for calls, views, events, compute runs and presence.

#[allow(
    clippy::wildcard_imports,
    reason = "the handlers moved here from `api.rs` read its items"
)]
use super::files::files_gate;
use super::*;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct IntentBody {
    method: String,
    #[serde(default)]
    args: Value,
    #[serde(default)]
    idempotency_key: Option<String>,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// The 422/503 of a component's run that did not end `Ok` (spec section 5, D23, D36).
pub(super) fn component_failure(
    index: u64,
    status: &Status,
    error: &Value,
    timeout: bool,
) -> Option<ApiError> {
    if timeout {
        return Some(crate::node::timeout_error());
    }
    match status {
        Status::Ok => None,
        Status::AppError => Some(
            ApiError::new(422, "APP_ERROR", "the app refused the call").with_details(json!({
                "index": index,
                "code": error["code"],
                "fields": error["fields"],
            })),
        ),
        Status::Trap => Some(
            ApiError::new(422, "TRAP", "the run trapped").with_details(json!({
                "index": index,
                "kind": error["kind"],
                "detail": error["detail"],
            })),
        ),
    }
}

/// A component call's answer. Its events are the ones the caller is in the
/// audience of (R9).
fn component_answer(
    node: &Node,
    app_id: &str,
    caller: &Caller,
    a: crate::component::Applied,
) -> Response {
    if a.conflict {
        return ApiError::unavailable(
            "the call raced a leader change; retry with the same idempotency key",
        )
        .into_response();
    }
    if a.error.as_str() == Some(crate::node::NOT_REACHED) {
        return ApiError::not_found(crate::node::NOT_REACHED).into_response();
    }
    if a.error.as_str() == Some(crate::node::READ_ONLY) {
        return ApiError::forbidden(crate::node::READ_ONLY).into_response();
    }
    if let Some(e) = component_failure(a.index, &a.status, &a.error, a.timeout) {
        return e.into_response();
    }
    let account = caller.account();
    let events: Vec<Value> = node.sys(|s| {
        let Some(app) = s.apps.get(app_id) else {
            return Vec::new();
        };
        a.events
            .iter()
            .filter(|e| s.receives(app, &e.audience, account, account))
            .map(|e| json!({ "kind": e.kind, "data": e.data }))
            .collect()
    });
    let receipt = node.receipt(app_id, a.index, &a.result);
    Json(json!({ "index": a.index, "result": a.result, "events": events, "receipt": receipt }))
        .into_response()
}

/// `/intents` for a component app: the ABI of the bundle the app group runs
/// (Ruling 40) decides whether it is a view and whether it needs a passkey.
async fn component_intents(
    node: &Node,
    app: &crate::types::AppRecord,
    p: Principal,
    body: IntentBody,
) -> Result<Response, ApiError> {
    app.require_app_component()?;
    let app_id = app.app_id.clone();
    let method = node
        .prepared_for(&node.running_bundle(app))
        .await?
        .abi
        .methods
        .get(&body.method)
        .cloned();
    let caller = p.caller.clone();
    // A declared view runs as one, whichever way it is invoked: it
    // reads the latest state and writes nothing.
    if method
        .as_ref()
        .is_some_and(|m| m.kind == calimero_app_host::MethodKind::View && !m.confirm)
    {
        let a = node
            .view(&ViewRequest {
                app_id: app_id.clone(),
                method: body.method,
                args: body.args,
                caller: p.caller,
                strong: true,
                min_index: 0,
            })
            .await?;
        return Ok(component_answer(
            node,
            &app_id,
            &caller,
            crate::component::Applied {
                index: a.index,
                status: a.status,
                result: a.result,
                error: a.error,
                events: Vec::new(),
                conflict: false,
                timeout: a.timeout,
            },
        ));
    }
    // Confirm flags come from the running bundle's ABI (S5), never from the app record.
    let consent_value = if method.as_ref().is_some_and(|m| m.confirm) {
        Some(consent(node, &p, &app_id, &body.method, &body.args, body.confirm).await?)
    } else {
        None
    };
    let unconfirmed = consent_value.is_none();
    let (method_name, args) = (body.method.clone(), body.args.clone());
    let applied = match node
        .call_component(&CallRequest {
            idempotency_key: body.idempotency_key,
            consent: consent_value,
            sys_index: p.sys_index,
            ..CallRequest::new(app_id.clone(), body.method, body.args, p.caller.clone())
        })
        .await
    {
        // The replica that runs it found a confirm method this node's ABI did not
        // know (it was stale): the step-up is the usual `412 CONFIRM_REQUIRED` with
        // a challenge (SDK reference).
        Err(e) if unconfirmed && e.details["code"] == "ConfirmRequired" => {
            return Err(consent(node, &p, &app_id, &method_name, &args, None)
                .await
                .err()
                .unwrap_or(e));
        }
        other => other?,
    };
    Ok(component_answer(node, &app_id, &caller, applied))
}

async fn intents(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<IntentBody>,
) -> Result<Response, ApiError> {
    not_reserved(&body.method)?;
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let p = gate(&node, &headers, &app, &body.method, true).await?;
    if app.executor != crate::types::Executor::Component {
        return Err(ApiError::unsupported_engine());
    }
    component_intents(&node, &app, p, body).await
}

/// The node's commands and the runtime's exports are not an app's methods
/// (`node::reserved_method`): a client naming one would otherwise reach
/// `__upgrade` or `__blob` past the API's authority checks.
fn not_reserved(method: &str) -> Result<(), ApiError> {
    if crate::node::reserved_method(method) {
        return Err(ApiError::bad_request(
            "RESERVED_METHOD",
            &format!("{method} is not a method of this app"),
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ViewBody {
    method: String,
    #[serde(default)]
    args: Value,
    #[serde(default)]
    consistency: Option<String>,
    #[serde(default)]
    min_index: u64,
}

async fn views(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<ViewBody>,
) -> Result<Response, ApiError> {
    not_reserved(&body.method)?;
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let p = gate(&node, &headers, &app, &body.method, false).await?;
    let a = node
        .view(&ViewRequest {
            app_id: app_id.clone(),
            method: body.method,
            args: body.args,
            caller: p.caller,
            strong: body.consistency.as_deref() != Some("stale"),
            min_index: body.min_index,
        })
        .await?;
    Ok(
        match component_failure(a.index, &a.status, &a.error, a.timeout) {
            Some(e) => e.into_response(),
            None => {
                let receipt = node.receipt(&app_id, a.index, &a.result);
                Json(json!({ "index": a.index, "result": a.result, "receipt": receipt }))
                    .into_response()
            }
        },
    )
}

#[derive(Deserialize)]
pub(super) struct EventsQuery {
    #[serde(default)]
    from: u64,
}

/// The most event streams one node serves at once (Xil's review, issue 12).
pub const MAX_EVENT_STREAMS: usize = 4096;

/// One open event stream, counted until the stream is dropped.
struct StreamSlot(Arc<std::sync::atomic::AtomicUsize>);

impl StreamSlot {
    fn take(node: &Node) -> Result<Self, ApiError> {
        use std::sync::atomic::Ordering;
        let open = node.event_streams.fetch_add(1, Ordering::SeqCst);
        let slot = Self(Arc::clone(&node.event_streams));
        if open >= MAX_EVENT_STREAMS {
            return Err(ApiError::new(
                429,
                "RATE_LIMITED",
                "this node serves as many event streams as it can; try another node or later",
            ));
        }
        Ok(slot)
    }
}

impl Drop for StreamSlot {
    fn drop(&mut self) {
        let _was = self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

async fn events(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Query(q): Query<EventsQuery>,
) -> Result<Response, ApiError> {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let p = gate(&node, &headers, &app, "*", false).await?;
    let subscriber = p.caller.account().map(str::to_owned);
    let slot = StreamSlot::take(&node)?;
    if let Some(stream) = local_events(&node, &app_id, q.from, subscriber.clone()) {
        let stream = stream.map(move |e| {
            let _held = &slot;
            e
        });
        return Ok(Sse::new(stream)
            .keep_alive(KeepAlive::default())
            .into_response());
    }
    let addr = node
        .sys(|s| {
            app.replicas
                .iter()
                .find_map(|r| s.nodes.get(r).map(|n| n.addr.clone()))
        })
        .ok_or_else(|| ApiError::unavailable("no replica known"))?;
    let resp = node
        .peer_stream(
            &addr,
            &format!(
                "internal/events?app={app_id}&from={}&subscriber={}",
                q.from,
                subscriber.unwrap_or_default()
            ),
        )
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

#[derive(Deserialize)]
pub(super) struct PresenceBody {
    seq: u64,
    /// Hex; `null` retracts.
    #[serde(default)]
    state: Option<String>,
}

/// Publish the caller's presence in an app (`presence-intents`): anyone
/// who reaches the app, one entry per device (the session's passkey, or the
/// API key). Re-send the same `seq` and state as a heartbeat.
async fn presence_publish(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<PresenceBody>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let p = gate(&node, &headers, &app, "*", false).await?;
    let account = p.account()?;
    if let Some(s) = &body.state {
        if hex::decode(s).is_err() || s.len() > 2 * crate::presence::MAX_STATE_BYTES {
            return Err(ApiError::bad_request(
                "BAD_STATE",
                "state is hex of at most 4 KiB",
            ));
        }
    }
    let device = match (&p.session, &p.api_key) {
        (Some(s), _) => s.passkey.clone().unwrap_or_else(|| s.session.clone()),
        (None, Some(k)) => format!("key:{}", k.key_id),
        (None, None) => String::new(),
    };
    node.publish_presence(crate::presence::Update {
        app: app_id,
        account,
        device,
        seq: body.seq,
        state: body.state,
    });
    Ok(Json(json!({})))
}

/// Who is present in an app now.
async fn presence_get(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let _p = gate(&node, &headers, &app, "*", false).await?;
    Ok(Json(json!({ "present": node.presence.snapshot(&app_id) })))
}

/// Presence as a stream: a `snapshot` event, then a `presence` event per change
/// (`state: null` when someone left).
async fn presence_stream(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> Result<Response, ApiError> {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let _p = gate(&node, &headers, &app, "*", false).await?;
    let slot = StreamSlot::take(&node)?;
    let (now, rx) = node.presence.subscribe(&app_id);
    let first = futures_util::stream::once(async move {
        Ok::<_, Infallible>(
            Event::default()
                .event("snapshot")
                .data(serde_json::to_string(&now).unwrap_or_default()),
        )
    });
    let live = tokio_stream::wrappers::BroadcastStream::new(rx)
        .filter_map(|r| async move { r.ok() })
        .map(move |d| {
            let _held = &slot;
            Ok(Event::default()
                .event("presence")
                .data(serde_json::to_string(&d).unwrap_or_default()))
        });
    Ok(Sse::new(first.chain(live))
        .keep_alive(KeepAlive::default())
        .into_response())
}

/// `POST /apps/{app}/methods/{method}`: the body of `/intents` or `/views` without
/// `method`, which the path names.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct MethodBody {
    #[serde(default)]
    args: Value,
    #[serde(default)]
    idempotency_key: Option<String>,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
    #[serde(default)]
    consistency: Option<String>,
    #[serde(default)]
    min_index: u64,
}

/// One route per method, as the app's OpenAPI document lists them: a view (one the ABI of
/// the running bundle declares a view, without `confirm`) is answered by [`views`], anything
/// else by [`intents`], each with its own gate, step-up and limits, so the two spellings of a
/// call behave the same. A method the ABI does not declare, or an ABI this node cannot read
/// yet, goes to `intents`, which refuses it as it would.
async fn app_method(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, method)): Path<(String, String)>,
    Json(body): Json<MethodBody>,
) -> Result<Response, ApiError> {
    not_reserved(&method)?;
    let app = node.app(&node.canonical_app_id(&app_id)).await?;
    let view = app.executor == crate::types::Executor::Component
        && node
            .prepared_for(&node.running_bundle(&app))
            .await
            .is_ok_and(|p| {
                p.abi
                    .methods
                    .get(&method)
                    .is_some_and(|m| m.kind == calimero_app_host::MethodKind::View && !m.confirm)
            });
    if view {
        let body = ViewBody {
            method,
            args: body.args,
            consistency: body.consistency,
            min_index: body.min_index,
        };
        return views(State(node), headers, Path(app_id), Json(body)).await;
    }
    let body = IntentBody {
        method,
        args: body.args,
        idempotency_key: body.idempotency_key,
        confirm: body.confirm,
    };
    intents(State(node), headers, Path(app_id), Json(body)).await
}

/// A compute run's record without its arguments or keys: whoever reaches the app may read it, but
/// file sizes, hashes, progress and the failure text only those who may list its files.
async fn app_run(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, run)): Path<(String, String)>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    if app.executor != crate::types::Executor::Component {
        return Err(ApiError::not_found("no such run"));
    }
    let p = gate(&node, &headers, &app, "*", false).await?;
    let files = node.sys(|s| may_handle_files(s, &app, p.caller.account()));
    let answer = node
        .view(&ViewRequest {
            app_id,
            method: crate::node::RUN_VIEW.to_owned(),
            args: json!({ "run": run, "files": files }),
            caller: p.caller,
            strong: true,
            min_index: 0,
        })
        .await?;
    if answer.result.is_null() {
        return Err(ApiError::not_found("no such run"));
    }
    Ok(Json(answer.result))
}

/// The module log of each attempt of a run that kept one, as `{ logs: [{ attempt, id, size,
/// sha256 }] }`, for those who may handle the app's files; the bytes are a files export.
async fn app_run_logs(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, run)): Path<(String, String)>,
) -> ApiResult {
    let (app, account) = files_gate(&node, &headers, &app_id, false).await?;
    let answer = node
        .view(&ViewRequest {
            app_id: app.app_id,
            method: crate::node::RUN_VIEW.to_owned(),
            args: json!({ "run": run, "files": true }),
            caller: Caller::Account { account },
            strong: true,
            min_index: 0,
        })
        .await?;
    if answer.result.is_null() {
        return Err(ApiError::not_found("no such run"));
    }
    Ok(Json(json!({ "logs": answer.result["logs"] })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RunsQuery {
    #[serde(default)]
    limit: Option<String>,
    #[serde(default)]
    before: Option<String>,
}

/// A page of an app's compute runs, newest first, each as `app_run` shows it, with the cursor of
/// the next page.
async fn app_runs(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Query(q): Query<RunsQuery>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    if app.executor != crate::types::Executor::Component {
        return Err(ApiError::not_found("no such run"));
    }
    let p = gate(&node, &headers, &app, "*", false).await?;
    let files = node.sys(|s| may_handle_files(s, &app, p.caller.account()));
    let limit = match q.limit {
        None => Value::Null,
        Some(l) => json!(l
            .parse::<u64>()
            .map_err(|_| { ApiError::bad_request("BAD_LIMIT", "limit is a whole number") })?),
    };
    let answer = node
        .view(&ViewRequest {
            app_id,
            method: crate::node::RUN_VIEW.to_owned(),
            args: json!({ "limit": limit, "before": q.before, "files": files }),
            caller: p.caller,
            strong: true,
            min_index: 0,
        })
        .await?;
    Ok(Json(answer.result))
}

/// This area's routes, merged into [`super::router`].
pub(super) fn routes() -> Router<Arc<Node>> {
    Router::new()
        .route("/apps/{app}/methods/{method}", post(app_method))
        .route("/apps/{app}/runs", get(app_runs))
        .route("/apps/{app}/runs/{run}", get(app_run))
        .route("/apps/{app}/runs/{run}/logs", get(app_run_logs))
        .route("/pipelines/validate", post(pipelines::validate))
        .route("/apps/{app}/pipelines/{name}/runs", post(pipelines::start))
        .route(
            "/apps/{app}/pipelines/{name}/estimate",
            post(pipelines::estimate_run),
        )
        .route("/apps/{app}/pipeline-runs", get(pipelines::list))
        .route("/apps/{app}/pipeline-runs/{run}", get(pipelines::get))
        .route(
            "/apps/{app}/pipeline-runs/{run}/cancel",
            post(pipelines::cancel),
        )
        .route(
            "/apps/{app}/pipeline-runs/{run}/receipt",
            get(pipeline_receipt::receipt),
        )
        .route(
            "/apps/{app}/pipeline-runs/{run}/events",
            get(pipeline_events::events),
        )
        .route(
            "/apps/{app}/pipeline-runs/{run}/retry",
            post(pipelines::retry),
        )
        .route(
            "/apps/{app}/presence",
            get(presence_get).post(presence_publish),
        )
        .route("/apps/{app}/presence/stream", get(presence_stream))
        .route("/apps/{app}/intents", post(intents))
        .route("/apps/{app}/views", post(views))
        .route("/apps/{app}/events", get(events))
}
