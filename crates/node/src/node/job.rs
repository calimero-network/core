//! The leader's side of a job node: attesting it and asking what it is running.

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use calimero_log::NodeId;
use calimero_sealed::client::{resolve_path, SealedTransport};
use calimero_tee_release::compare_release_versions;
use futures_util::future::join_all;
use reqwest::Method;
use serde::Deserialize;
use url::Url;

use super::Node;
use crate::api::PLACEMENT_RELEASE_WAIT;
use crate::attest::{verify_job_key, AttestationDoc};
use crate::error::ApiError;
use crate::job_node::CANCEL_WAIT;
use crate::job_wire::{self, Assignment};
use crate::types::AppRecord;

/// How long an attested job node is trusted before it is attested again.
const JOB_ATTEST_TTL: Duration = Duration::from_secs(60);

/// The most a job node's answer to the leader may weigh (an attestation, a load report).
const MAX_JOB_RESPONSE_BYTES: usize = 64 * 1024;

/// The most a run's status may weigh: a signed report with its result and outputs.
pub(crate) const MAX_JOB_STATUS_BYTES: usize = 1024 * 1024;

/// How many transfers to job nodes one node has under way at once.
pub const MAX_INFLIGHT_JOB_OPS: usize = 8;

/// How many control requests to job nodes one node has under way at once, apart from transfers.
pub(super) const MAX_INFLIGHT_JOB_CONTROL: usize = 32;

/// This node's own queue for control requests was full: nothing is known of the job node.
#[derive(Debug)]
pub(crate) struct BusyHere;

impl std::fmt::Display for BusyHere {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("no free place for a control request to a job node")
    }
}

impl std::error::Error for BusyHere {}

/// Whether `e` is [`BusyHere`].
pub(crate) fn busy_here(e: &eyre::Report) -> bool {
    e.downcast_ref::<BusyHere>().is_some()
}

/// A job node did not answer in time.
#[derive(Debug)]
pub(crate) struct TimedOut(NodeId);

impl std::fmt::Display for TimedOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "job node {} did not answer in time", self.0)
    }
}

impl std::error::Error for TimedOut {}

/// Whether `e` is [`TimedOut`].
pub(crate) fn timed_out(e: &eyre::Report) -> bool {
    e.downcast_ref::<TimedOut>().is_some()
}

/// What a job node with no room answers.
const BUSY_STATUS: u16 = 503;

/// How long a status poll, delete, load or output-key request to a job node may take, its wait for a place included: a frozen node cannot stall a round.
pub const JOB_CONTROL_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a delete may take: the job node waits up to [`CANCEL_WAIT`] for the run to stop before it answers.
pub const JOB_DELETE_TIMEOUT: Duration =
    Duration::from_secs(CANCEL_WAIT.as_secs() + JOB_CONTROL_TIMEOUT.as_secs());

/// How long an attestation fetch from a job node may take: quote generation can be slow.
const JOB_ATTEST_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a request to a job node that moves a component or a file may take.
const JOB_TRANSFER_TIMEOUT: Duration = Duration::from_secs(300);

/// What a job node answered: its status, and its body read up to the caller's limit.
pub(crate) struct JobAnswer {
    pub status: u16,
    pub body: Vec<u8>,
}

impl JobAnswer {
    /// The body as JSON.
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> eyre::Result<T> {
        Ok(serde_json::from_slice(&self.body)?)
    }

    /// The `error` code of a refusal.
    pub fn code(&self) -> Option<String> {
        let refusal: crate::error::ApiError = self.json().ok()?;
        Some(refusal.code)
    }
}

/// A job node's answer was larger than the leader reads.
#[derive(Debug, thiserror::Error)]
#[error("the job node's answer is larger than {0} bytes")]
pub(crate) struct AnswerTooLarge(pub usize);

/// What a job node answered, refused past `max` bytes before it is held whole.
async fn bounded_body(mut response: reqwest::Response, max: usize) -> eyre::Result<Vec<u8>> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > max {
            return Err(AnswerTooLarge(max).into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn bounded_json<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> eyre::Result<T> {
    Ok(serde_json::from_slice(
        &bounded_body(response, MAX_JOB_RESPONSE_BYTES).await?,
    )?)
}

/// A job node whose transport and job signing keys one quote bound.
#[derive(Clone, Debug)]
pub struct JobPeer {
    pub id: NodeId,
    pub url: String,
    pub transport_key: [u8; 32],
    pub job_key: [u8; 32],
    /// The mero-tee release its attestation named.
    pub release: String,
    /// The sealed channel pinned to `transport_key`.
    pub channel: Arc<SealedTransport>,
}

#[derive(Deserialize)]
struct Load {
    running: usize,
    capacity: usize,
}

impl Node {
    /// Attest job node `id` and check that its release may run `app`'s work and is not retired
    /// (on every call; the attestation itself is kept, see `job_attestation`).
    pub async fn attest_job(&self, id: NodeId, app: &AppRecord) -> eyre::Result<JobPeer> {
        let peer = self.job_attestation(id).await?;
        if !self.sys(|s| s.release_may_hold(app, Some(&peer.release))) {
            eyre::bail!(
                "job node {id} runs release {}, which this app's owner has not approved",
                peer.release
            );
        }
        let now = self.clock.now_ms() / 1000;
        if self.sys(|s| s.retired(&peer.release, now)) {
            eyre::bail!(
                "job node {id} runs release {}, which is retired by a deadline",
                peer.release
            );
        }
        Ok(peer)
    }

    /// Job node `id` as its attestation shows it, kept for [`JOB_ATTEST_TTL`]
    /// and only while the directory still holds the registration it was made for.
    pub(crate) async fn job_attestation(&self, id: NodeId) -> eyre::Result<JobPeer> {
        if !self.sys_fresh().await {
            eyre::bail!("this node's directory could not be brought up to date");
        }
        let (url, epoch) = self
            .sys(|s| {
                let url = s.job_nodes().find(|n| n.id == id)?.addr.clone();
                Some((url, s.node_epoch.get(&id).copied().unwrap_or(0)))
            })
            .ok_or_else(|| eyre::eyre!("node {id} is not a registered job node"))?;
        let cached = self
            .job_peers
            .lock()
            .map_err(|e| eyre::eyre!("{e}"))?
            .get(&id)
            .filter(|(at, kept, p)| at.elapsed() < JOB_ATTEST_TTL && *kept == epoch && p.url == url)
            .map(|(_, _, p)| p.clone());
        if let Some(p) = cached {
            return Ok(p);
        }
        let peer = self.fetch_job_attestation(id, &url).await?;
        let _prev = self
            .job_peers
            .lock()
            .map_err(|e| eyre::eyre!("{e}"))?
            .insert(id, (Instant::now(), epoch, peer.clone()));
        Ok(peer)
    }

    /// The cluster's identity key (public half): what a job node is started with to take this
    /// cluster's work.
    #[must_use]
    pub fn cluster_identity(&self) -> [u8; 32] {
        crate::backup::cluster_identity_key(&self.cluster_key)
            .verifying_key()
            .to_bytes()
    }

    /// The cluster identity's signature (hex) over `assignment`.
    pub fn sign_assignment(&self, assignment: &Assignment) -> eyre::Result<String> {
        job_wire::sign_assignment(
            &crate::backup::cluster_identity_key(&self.cluster_key),
            assignment,
        )
    }

    /// The cluster identity's signature (hex) that lets a job node take the component `hash`.
    pub fn sign_bundle(&self, hash: &str) -> eyre::Result<String> {
        job_wire::sign_bundle(
            &crate::backup::cluster_identity_key(&self.cluster_key),
            hash,
        )
    }

    /// Drops what is kept for job node `id`: every failure of a channel to it goes through here.
    pub(crate) fn forget_job(&self, id: NodeId) {
        if let Ok(mut peers) = self.job_peers.lock() {
            let _gone = peers.remove(&id);
        }
    }

    async fn fetch_job_attestation(&self, id: NodeId, url: &str) -> eyre::Result<JobPeer> {
        let nonce: [u8; 32] = calimero_identity::random();
        let mut at = resolve_path(&url.parse::<Url>()?, "attestation")?;
        let _q = at
            .query_pairs_mut()
            .append_pair("nonce", &hex::encode(nonce))
            .append_pair("purpose", "jobs");
        let doc: AttestationDoc = bounded_json(
            self.http
                .get(at)
                .timeout(JOB_ATTEST_TIMEOUT)
                .send()
                .await?
                .error_for_status()?,
        )
        .await?;
        if doc.cluster != self.config.cluster || doc.node != id {
            eyre::bail!(
                "job node {id} attests as node {} of cluster {}",
                doc.node,
                doc.cluster
            );
        }
        let (transport_key, job_key) =
            verify_job_key(&doc, &nonce, &self.config.peer_policy()).await?;
        // The quote binds the identity the job node takes work from: not this cluster's, no work.
        if doc.cluster_identity.as_deref() != Some(hex::encode(self.cluster_identity()).as_str()) {
            eyre::bail!("job node {id} takes work from another cluster");
        }
        Ok(JobPeer {
            id,
            url: url.to_owned(),
            transport_key,
            job_key,
            release: doc.release.version,
            channel: Arc::new(SealedTransport::with_key(
                url.parse()?,
                self.http.clone(),
                transport_key,
            )),
        })
    }

    /// How many runs `peer` has and how many it takes, asked over a sealed
    /// channel to the transport key its attestation bound.
    pub async fn job_load(&self, peer: &JobPeer) -> eyre::Result<(usize, usize)> {
        let asked = self
            .job_call(
                peer,
                Method::GET,
                "job/load",
                &[],
                Vec::new(),
                MAX_JOB_RESPONSE_BYTES,
            )
            .await
            .and_then(|answer| {
                let load: Load = answer.json()?;
                Ok((load.running, load.capacity))
            });
        if asked.as_ref().is_err_and(|e| !busy_here(e)) {
            self.forget_job(peer.id);
        }
        asked
    }

    /// A control request to `peer`'s `path` over its sealed channel, its body read up to `max`
    /// bytes, within [`JOB_CONTROL_TIMEOUT`]. A failed request or a 5xx drops what is kept of the node.
    pub(crate) async fn job_call(
        &self,
        peer: &JobPeer,
        method: Method,
        path: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
        max: usize,
    ) -> eyre::Result<JobAnswer> {
        self.job_call_within(JOB_CONTROL_TIMEOUT, peer, method, path, headers, body, max)
            .await
    }

    /// [`Self::job_call`] with `timeout` for the request itself.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn job_call_within(
        &self,
        timeout: Duration,
        peer: &JobPeer,
        method: Method,
        path: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
        max: usize,
    ) -> eyre::Result<JobAnswer> {
        let waited = tokio::time::timeout(JOB_CONTROL_TIMEOUT, self.job_control.acquire()).await;
        let Ok(slot) = waited else {
            return Err(BusyHere.into());
        };
        let _slot = slot?;
        let call = self.call_job(peer, method, path, headers, body, max);
        self.within(timeout, peer, call).await
    }

    /// `call` within `timeout`; a node that does not answer in time is dropped.
    async fn within<T>(
        &self,
        timeout: Duration,
        peer: &JobPeer,
        call: impl std::future::Future<Output = eyre::Result<T>>,
    ) -> eyre::Result<T> {
        if let Ok(done) = tokio::time::timeout(timeout, call).await {
            return done;
        }
        self.forget_job(peer.id);
        Err(TimedOut(peer.id).into())
    }

    /// [`Self::job_call`] for a request that moves a component, an assignment or a file, with
    /// the transfer timeout.
    pub(crate) async fn job_transfer(
        &self,
        peer: &JobPeer,
        method: Method,
        path: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
        max: usize,
    ) -> eyre::Result<JobAnswer> {
        let _slot = self.job_slot().await?;
        self.job_call_held(peer, method, path, headers, body, max)
            .await
    }

    /// A place among the operations to job nodes under way; hold it while holding bytes that
    /// are meant for one.
    pub(crate) async fn job_slot(&self) -> eyre::Result<tokio::sync::SemaphorePermit<'_>> {
        Ok(self.job_ops.acquire().await?)
    }

    /// [`Self::job_transfer`] by a caller that holds a [`Self::job_slot`] already.
    pub(crate) async fn job_call_held(
        &self,
        peer: &JobPeer,
        method: Method,
        path: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
        max: usize,
    ) -> eyre::Result<JobAnswer> {
        let call = self.call_job(peer, method, path, headers, body, max);
        self.within(JOB_TRANSFER_TIMEOUT, peer, call).await
    }

    /// One request. A busy job node (503) is not a failed channel.
    async fn call_job(
        &self,
        peer: &JobPeer,
        method: Method,
        path: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
        max: usize,
    ) -> eyre::Result<JobAnswer> {
        let sent = self.send_job(peer, method, path, headers, body, max).await;
        if sent
            .as_ref()
            .map_or(true, |a| a.status >= 500 && a.status != BUSY_STATUS)
        {
            self.forget_job(peer.id);
        }
        let answer = sent?;
        if answer.status >= 500 {
            eyre::bail!("job node {} answered {}", peer.id, answer.status);
        }
        Ok(answer)
    }

    async fn send_job(
        &self,
        peer: &JobPeer,
        method: Method,
        path: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
        max: usize,
    ) -> eyre::Result<JobAnswer> {
        let url = resolve_path(&peer.url.parse()?, path)?;
        let mut request = self.http.request(method, url);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        if !body.is_empty() {
            request = request.body(body);
        }
        let response = peer.channel.execute(request.build()?).await?;
        let status = response.status().as_u16();
        Ok(JobAnswer {
            status,
            body: bounded_body(response, max).await?,
        })
    }
}

/// Whether a node on `release` is at least `own`. A release that cannot be
/// read or compared is not.
fn release_at_least(release: Option<&str>, own: &str) -> bool {
    release.is_some_and(|r| {
        r == own || compare_release_versions(r, own).is_some_and(|o| o != Ordering::Less)
    })
}

impl Node {
    /// Refuses unless every replica node, voter and learner runs at least this
    /// node's release: no older node may read a job record.
    pub(crate) async fn require_upgraded_nodes(&self) -> Result<(), ApiError> {
        let own = self.own_release();
        let status = self.system.status();
        let ids: BTreeSet<NodeId> = self
            .sys(|s| s.replica_nodes().map(|n| n.id).collect::<Vec<_>>())
            .into_iter()
            .chain(status.voters)
            .chain(status.learners)
            .collect();
        let releases = join_all(
            ids.iter()
                .map(|id| tokio::time::timeout(PLACEMENT_RELEASE_WAIT, self.release_of_node(*id))),
        )
        .await;
        for (id, release) in ids.iter().zip(releases) {
            if !release_at_least(release.ok().flatten().as_deref(), &own) {
                return Err(ApiError::new(
                    409,
                    "CLUSTER_NOT_UPGRADED",
                    &format!("node {id} does not run release {own} or later yet: upgrade it first"),
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::release_at_least;

    #[test]
    fn a_release_is_at_least_the_own_one_only_when_it_can_be_read_and_is_not_older() {
        assert!(release_at_least(Some("2.4.0"), "2.4.0"));
        assert!(release_at_least(Some("2.5.0"), "2.4.0"));
        assert!(release_at_least(Some("test"), "test"));
        assert!(!release_at_least(Some("2.3.9"), "2.4.0"));
        assert!(!release_at_least(Some("2.4.0-rc.1"), "2.4.0"));
        assert!(!release_at_least(Some("test"), "2.4.0"));
        assert!(!release_at_least(None, "2.4.0"));
    }
}
