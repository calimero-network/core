//! Each node's runtime, as it answers it, and the runtime a release needs
//! ([`crate::runtime`]).

use std::collections::BTreeMap;
use std::time::Duration;

use calimero_log::NodeId;
use semver::Version;
use serde_json::Value;
use tracing::debug;

use super::{Node, PEER_NODE_HEADER};
use crate::error::ApiError;
use crate::runtime::{NodeRuntime, MAX_RUNTIME_ANSWER_BYTES};
use crate::system::PackageRef;

/// How long a peer's runtime is kept before it is asked again. A node changes
/// its runtime only by restarting on another release, so for this long after
/// that it may be judged by the one before.
pub const RUNTIME_KEPT: Duration = Duration::from_secs(30);

/// The most a node waits for a peer to name its runtime; one that does not
/// answer by then is [`NodeRuntime::Unknown`].
pub const RUNTIME_WAIT: Duration = Duration::from_secs(2);

impl Node {
    /// This node's runtime: the release it was built as (`Config::release`).
    #[must_use]
    pub fn own_runtime(&self) -> NodeRuntime {
        NodeRuntime::named(&self.config.release)
    }

    /// The runtime node `id` names over the sealed peer channel (an
    /// attested node's own answer), kept [`RUNTIME_KEPT`].
    pub async fn runtime_of_node(&self, id: NodeId) -> NodeRuntime {
        if id == self.config.node_id {
            return self.own_runtime();
        }
        let now = tokio::time::Instant::now();
        let kept = self
            .peer_runtimes
            .lock()
            .ok()
            .and_then(|k| k.get(&id).cloned())
            .filter(|(at, _)| now.duration_since(*at) < RUNTIME_KEPT);
        if let Some((_, rt)) = kept {
            return rt;
        }
        let Some(addr) = self.sys(|s| s.nodes.get(&id).map(|n| n.addr.clone())) else {
            return NodeRuntime::Unknown;
        };
        let Ok(Some(rt)) = tokio::time::timeout(RUNTIME_WAIT, self.ask_runtime(&addr)).await else {
            debug!(node = id, "a peer did not name its runtime");
            return NodeRuntime::Unknown;
        };
        if let Ok(mut k) = self.peer_runtimes.lock() {
            let _prev = k.insert(id, (now, rt.clone()));
        }
        rt
    }

    /// Ask the node at `addr` its runtime (`POST /internal/runtime`). A node
    /// that does not know the route answers `404`: it runs a release from
    /// before it, older than this node's. `None` when there is no answer.
    async fn ask_runtime(&self, addr: &str) -> Option<NodeRuntime> {
        let peer = self.connected(addr).await.ok()?;
        let url =
            calimero_sealed::client::resolve_path(&addr.parse().ok()?, "internal/runtime").ok()?;
        let req = self
            .http
            .post(url)
            .header("x-cluster-auth", &self.cluster_secret)
            .header(PEER_NODE_HEADER, self.config.node_id.to_string())
            .build()
            .ok()?;
        let mut resp = peer.execute(req).await.ok()?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Some(match self.own_runtime() {
                NodeRuntime::Known(own) => NodeRuntime::Before(own),
                _ => NodeRuntime::Unknown,
            });
        }
        if !resp.status().is_success() {
            return None;
        }
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await.ok()? {
            body.extend_from_slice(&chunk);
            if body.len() > MAX_RUNTIME_ANSWER_BYTES {
                return None;
            }
        }
        let v: Value = serde_json::from_slice(&body).ok()?;
        Some(
            v["runtime"]
                .as_str()
                .map_or(NodeRuntime::Unknown, NodeRuntime::named),
        )
    }

    /// The runtime of each of `ids`, asked at once.
    pub async fn runtimes(&self, ids: &[NodeId]) -> BTreeMap<NodeId, NodeRuntime> {
        let named =
            futures_util::future::join_all(ids.iter().map(|id| self.runtime_of_node(*id))).await;
        ids.iter().copied().zip(named).collect()
    }

    /// The runtime every node of `ids` must run to hold the release
    /// `release`, or else the release `bundle` comes out of: its
    /// `minRuntimeVersion`. `None` for a raw development bundle, which has no
    /// manifest, and for a release whose `.mpk` cannot be read now.
    pub async fn min_runtime_of(
        &self,
        release: Option<&PackageRef>,
        bundle: &str,
    ) -> Option<Version> {
        let found = match release {
            Some(r) => self
                .sys(|s| {
                    s.packages
                        .get(&r.package)?
                        .versions
                        .get(&r.version)
                        .cloned()
                })
                .map(|v| (r.package.clone(), v)),
            None => self.release_of(bundle).map(|r| (r.at.package, r.version)),
        };
        let (package, v) = found?;
        if let Some(min) = &v.min_runtime_version {
            return crate::runtime::required(min);
        }
        // Recorded before releases kept it, or by a node on an earlier release.
        match crate::consent_page::declared_min_runtime(self, &package, &v).await {
            Ok(min) => crate::runtime::required(&min),
            Err(e) => {
                debug!(%package, version = %v.version, error = %e.message, "a release's runtime requirement could not be read");
                None
            }
        }
    }

    /// `409 RUNTIME_TOO_OLD` when a node of `nodes` runs a runtime older than
    /// the release `release` (or `bundle`'s) needs; `what` names the release
    /// in the message.
    pub async fn require_runtime(
        &self,
        nodes: &[NodeId],
        release: Option<&PackageRef>,
        bundle: &str,
        what: &str,
    ) -> Result<(), ApiError> {
        let Some(min) = self.min_runtime_of(release, bundle).await else {
            return Ok(());
        };
        let runtimes = self.runtimes(nodes).await;
        let short = crate::runtime::short_of(nodes, &runtimes, &min);
        if short.is_empty() {
            return Ok(());
        }
        Err(crate::runtime::refusal(what, &min, short))
    }
}
