//! The node's side of the cluster version ([`crate::system::cluster_version`]):
//! what each node supports, the gate every proposal goes through, raising the
//! logged version on the system leader, and keeping older nodes out of the
//! system group once it is raised.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use calimero_log::NodeId;
use futures_util::future::join_all;
use reqwest::Method;
use serde_json::Value;
use tracing::{info, warn};

use super::Node;
use crate::api::{FEATURE_NOT_ACTIVE, PLACEMENT_RELEASE_WAIT};
use crate::error::ApiError;
use crate::system::cluster_version::{self as cv, Feature};
use crate::system::SystemCmd;

/// How often the system leader asks the nodes what they support while the
/// logged cluster version is below this build's.
const RAISE_EVERY: Duration = Duration::from_secs(10);

/// The system leader's own bookkeeping for the cluster version.
#[derive(Default)]
pub(super) struct Raising {
    /// When it next asks the nodes which version they support.
    due: Option<tokio::time::Instant>,
    /// Registered nodes it did not admit for supporting less (logged once each).
    behind: BTreeSet<NodeId>,
}

impl Node {
    /// The cluster version node `id` supports, as its `/health` says over its
    /// sealed channel (this node's own for itself), waiting at most
    /// [`PLACEMENT_RELEASE_WAIT`]. `None` when it does not answer.
    async fn advertised_version(&self, id: NodeId) -> Option<String> {
        if id == self.config.node_id {
            return Some(cv::supported().to_owned());
        }
        let addr = self.sys(|s| s.nodes.get(&id).map(|n| n.addr.clone()))?;
        self.advertised_at(&addr).await
    }

    /// [`Self::advertised_version`] of the node at `addr`.
    async fn advertised_at(&self, addr: &str) -> Option<String> {
        let health = async {
            let resp = self
                .peer_raw(addr, Method::GET, "health", None)
                .await
                .ok()?;
            resp.json::<Value>().await.ok()
        };
        let health = tokio::time::timeout(PLACEMENT_RELEASE_WAIT, health)
            .await
            .ok()
            .flatten()?;
        cv::advertised(&health)
    }

    /// What every node the system log reaches (each registered replica node
    /// and each member of the system group) supports now, by id.
    #[doc(hidden)]
    pub async fn advertised_versions(&self) -> BTreeMap<NodeId, Option<String>> {
        let status = self.system.status();
        let mut ids: Vec<NodeId> = self.sys(|s| {
            s.replica_nodes()
                .map(|n| n.id)
                .filter(|id| !s.removed_nodes.contains(id))
                .collect()
        });
        ids.extend(status.voters.iter().chain(&status.learners));
        ids.sort_unstable();
        ids.dedup();
        let versions = join_all(ids.iter().map(|id| self.advertised_version(*id))).await;
        ids.into_iter().zip(versions).collect()
    }

    /// The gate every proposal goes through ([`Node::sys_write`]): `409
    /// FEATURE_NOT_ACTIVE` unless every feature is active, judged on the
    /// logged cluster version, or, when that is below, on what every node
    /// supports now.
    pub async fn require_features(&self, features: &[Feature]) -> Result<(), ApiError> {
        if features.is_empty() {
            return Ok(());
        }
        let logged = self.sys(|s| s.cluster_version.clone());
        if cv::check(logged.as_deref(), features).is_ok() {
            return Ok(());
        }
        let advertised = self.advertised_versions().await;
        let floor = cv::floor(advertised.values().map(Option::as_deref));
        let active = cv::effective(logged.as_deref(), floor.as_deref());
        cv::check(active.as_deref(), features).map_err(|refused| {
            ApiError::new(409, FEATURE_NOT_ACTIVE, &refused.message())
                .with_details(refused.details())
        })
    }

    /// On the system leader: log the cluster version every registered node
    /// supports once it is above the logged one (`SetClusterVersion`). Asks
    /// the nodes at most every [`RAISE_EVERY`], in the background, and not at
    /// all once the logged version is this build's.
    pub(super) fn raise_cluster_version(&self) {
        let supported = cv::supported();
        if self.sys(|s| s.cluster_version.as_deref() == Some(supported)) {
            return;
        }
        let now = tokio::time::Instant::now();
        let Ok(mut raising) = self.cluster_version.lock() else {
            return;
        };
        if raising.due.is_some_and(|d| now < d) {
            return;
        }
        raising.due = Some(now + RAISE_EVERY);
        drop(raising);
        let Some(me) = self.this.lock().ok().and_then(|w| w.upgrade()) else {
            return;
        };
        drop(tokio::spawn(async move {
            match me.raise_cluster_version_once().await {
                Ok(Some(version)) => info!(%version, "the cluster version was raised"),
                Ok(None) => {}
                Err(why) => warn!(%why, "could not raise the cluster version yet"),
            }
        }));
    }

    /// One pass of [`Self::raise_cluster_version`]: ask every node, and log
    /// the oldest version they support when it is above the logged one.
    /// `Ok(None)` when there is nothing to raise yet; `Err` says why a raise
    /// was refused.
    #[doc(hidden)]
    pub async fn raise_cluster_version_once(&self) -> Result<Option<String>, String> {
        let advertised = self.advertised_versions().await;
        let Some(version) = cv::floor(advertised.values().map(Option::as_deref)) else {
            return Err(format!("not every node answered: {advertised:?}"));
        };
        let logged = self.sys(|s| s.cluster_version.clone());
        if cv::effective(logged.as_deref(), Some(version.as_str())) == logged {
            return Ok(None);
        }
        // Below the version that brings `SetClusterVersion`, a node would
        // skip it: the floor judges proposals until every node has it.
        if let Err(refused) = cv::check(Some(version.as_str()), &[Feature::ClusterVersion]) {
            return Err(refused.message());
        }
        let registered: Vec<NodeId> = self.sys(|s| {
            s.replica_nodes()
                .map(|n| n.id)
                .filter(|id| !s.removed_nodes.contains(id))
                .collect()
        });
        let nodes: BTreeMap<u64, String> = registered
            .into_iter()
            .filter_map(|id| advertised.get(&id).cloned().flatten().map(|v| (id, v)))
            .collect();
        let _out = self
            .sys_write(&SystemCmd::SetClusterVersion {
                version: version.clone(),
                nodes: nodes.clone(),
            })
            .await
            .map_err(|e| format!("{} {}: {} (nodes {nodes:?})", e.status, e.code, e.message))?;
        Ok(Some(version))
    }

    /// Whether the registered node `id` at `addr` may join the system group
    /// at the logged cluster version ([`cv::may_join`]); one that may not is
    /// logged and tried again on the next pass.
    pub(super) async fn joins_at_cluster_version(&self, id: NodeId, addr: &str) -> bool {
        let Some(logged) = self.sys(|s| s.cluster_version.clone()) else {
            return true;
        };
        let advertised = self.advertised_at(addr).await;
        let admitted = cv::may_join(Some(&logged), advertised.as_deref());
        let Ok(mut raising) = self.cluster_version.lock() else {
            return admitted;
        };
        if admitted {
            let _was = raising.behind.remove(&id);
            return true;
        }
        // A node still starting does not answer yet; one that answers less
        // is told about once.
        if advertised.is_some() && raising.behind.insert(id) {
            warn!(
                node = id,
                advertised = advertised.as_deref().unwrap_or_default(),
                cluster_version = %logged,
                "a registered node supports less than the cluster version: not admitted"
            );
        }
        false
    }
}
