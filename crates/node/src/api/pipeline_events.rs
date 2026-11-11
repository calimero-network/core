//! `GET /apps/{app}/pipeline-runs/{run}/events`: a run's progress as server-sent events, metadata
//! only (`calimero_client::pipeline_events`). Whoever may read the run may subscribe.
//! Changes come from the leader's `__pipeline_run` view, polled and diffed, so a follower
//! answers as it does for `GET .../pipeline-runs/{run}`: through the leader.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use calimero_client::pipeline_events::{PipelineEvent, RunProgress};
use futures_util::Stream;
use serde_json::json;

use super::gate;
use super::pipelines::component_app;
use crate::error::ApiError;
use crate::node::{Node, ViewRequest, PIPELINE_VIEW};

/// Time between looks at the run.
pub const PIPELINE_EVENTS_POLL: Duration = Duration::from_millis(500);
/// The longest one stream lasts; the client reconnects and gets a new snapshot.
pub const PIPELINE_EVENTS_MAX_SECS: u64 = 3600;
/// Keep-alive comments, so a proxy does not drop a quiet stream.
const KEEP_ALIVE: Duration = Duration::from_secs(15);
/// Streams one app's runs may have open on a node.
pub const MAX_RUN_SUBSCRIBERS_PER_APP: usize = 64;
const TOO_MANY_SUBSCRIBERS: &str = "TOO_MANY_SUBSCRIBERS";

/// One open stream of an app, counted until the stream is dropped.
struct Subscriber {
    node: Arc<Node>,
    app: String,
}

impl Subscriber {
    fn take(node: &Arc<Node>, app: &str) -> Result<Self, ApiError> {
        let mut open = node
            .pipeline_subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let n = open.entry(app.to_owned()).or_default();
        if *n >= MAX_RUN_SUBSCRIBERS_PER_APP {
            return Err(ApiError::new(
                429,
                TOO_MANY_SUBSCRIBERS,
                "this app's pipeline runs have as many watchers as this node serves; try another node or later",
            ));
        }
        *n += 1;
        Ok(Self {
            node: Arc::clone(node),
            app: app.to_owned(),
        })
    }
}

impl Drop for Subscriber {
    fn drop(&mut self) {
        let mut open = self
            .node
            .pipeline_subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(n) = open.get_mut(&self.app) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                let _gone = open.remove(&self.app);
            }
        }
    }
}

struct Watch {
    node: Arc<Node>,
    req: ViewRequest,
    prev: Option<RunProgress>,
    queued: VecDeque<PipelineEvent>,
    closing: bool,
    first: bool,
    deadline: Instant,
    _slot: Subscriber,
}

impl Watch {
    /// The next event, looking at the run again until something moved; `None` ends the stream.
    async fn next(&mut self) -> Option<PipelineEvent> {
        loop {
            if let Some(e) = self.queued.pop_front() {
                return Some(e);
            }
            if self.closing || Instant::now() >= self.deadline {
                return None;
            }
            if !self.first {
                tokio::time::sleep(PIPELINE_EVENTS_POLL).await;
            }
            self.first = false;
            // A view the leader could not answer this time (an election) is tried again.
            let Ok(answer) = self.node.view(&self.req).await else {
                continue;
            };
            let now = RunProgress::from_view(&answer.result)?;
            match &self.prev {
                None => self.queued.push_back(now.snapshot()),
                Some(prev) => self.queued.extend(now.diff(prev)),
            }
            self.closing = now.ended();
            self.prev = Some(now);
        }
    }
}

fn frame(e: &PipelineEvent) -> Result<Event, Infallible> {
    Ok(Event::default().event(e.name()).data(e.data()))
}

pub(super) async fn events(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, run)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let app_id = node.canonical_app_id(&app_id);
    let app = component_app(&node, &app_id).await?;
    let p = gate(&node, &headers, &app, "*", false).await?;
    let slot = Subscriber::take(&node, &app_id)?;
    let req = ViewRequest {
        app_id,
        method: PIPELINE_VIEW.to_owned(),
        args: json!({ "run": run, "files": false }),
        caller: p.caller,
        strong: true,
        min_index: 0,
    };
    let first = node.view(&req).await?;
    if first.result.is_null() {
        return Err(ApiError::not_found("no such pipeline run"));
    }
    let watch = Watch {
        node,
        req,
        prev: None,
        queued: VecDeque::new(),
        closing: false,
        first: true,
        deadline: Instant::now() + Duration::from_secs(PIPELINE_EVENTS_MAX_SECS),
        _slot: slot,
    };
    let stream = futures_util::stream::unfold(watch, |mut w| async move {
        let e = w.next().await?;
        Some((frame(&e), w))
    });
    Ok(sse(stream).into_response())
}

fn sse(
    stream: impl Stream<Item = Result<Event, Infallible>> + Send + 'static,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    Sse::new(stream).keep_alive(KeepAlive::new().interval(KEEP_ALIVE))
}
