//! The leader's side of a job node: attesting it and asking what it is running.

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use calimero_log::NodeId;
use calimero_sealed::client::{resolve_path, SealedTransport};
use calimero_tee_release::compare_release_versions;
use futures_util::future::join_all;
use serde::Deserialize;
use url::Url;

use super::Node;
use crate::api::PLACEMENT_RELEASE_WAIT;
use crate::attest::{verify_job_key, AttestationDoc};
use crate::error::ApiError;
use crate::job_wire::{self, Assignment};
use crate::types::AppRecord;

/// How long an attested job node is trusted before it is attested again.
const JOB_ATTEST_TTL: Duration = Duration::from_secs(60);

/// The most a job node's answer to the leader may weigh (an attestation, a load report).
const MAX_JOB_RESPONSE_BYTES: usize = 64 * 1024;

/// What a job node answered, refused past [`MAX_JOB_RESPONSE_BYTES`] before it is held whole.
async fn bounded_json<T: serde::de::DeserializeOwned>(
    mut response: reqwest::Response,
) -> eyre::Result<T> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > MAX_JOB_RESPONSE_BYTES {
            eyre::bail!("the job node's answer is larger than {MAX_JOB_RESPONSE_BYTES} bytes");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&body)?)
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
}

#[derive(Deserialize)]
struct Load {
    running: usize,
    capacity: usize,
}

impl Node {
    /// Attest job node `id` and check that its release may run `app`'s work
    /// (on every call; the attestation itself is kept, see `job_attestation`).
    pub async fn attest_job(&self, id: NodeId, app: &AppRecord) -> eyre::Result<JobPeer> {
        let peer = self.job_attestation(id).await?;
        if !self.sys(|s| s.release_may_hold(app, Some(&peer.release))) {
            eyre::bail!(
                "job node {id} runs release {}, which this app's owner has not approved",
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
        let doc: AttestationDoc =
            bounded_json(self.http.get(at).send().await?.error_for_status()?).await?;
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
        })
    }

    /// How many runs `peer` has and how many it takes, asked over a sealed
    /// channel to the transport key its attestation bound.
    pub async fn job_load(&self, peer: &JobPeer) -> eyre::Result<(usize, usize)> {
        let asked = self.ask_job_load(peer).await;
        if asked.is_err() {
            self.forget_job(peer.id);
        }
        asked
    }

    async fn ask_job_load(&self, peer: &JobPeer) -> eyre::Result<(usize, usize)> {
        let base: Url = peer.url.parse()?;
        let channel =
            SealedTransport::with_key(base.clone(), self.http.clone(), peer.transport_key);
        let url = resolve_path(&base, "job/load")?;
        let resp = channel.execute(self.http.get(url).build()?).await?;
        let load: Load = bounded_json(resp.error_for_status()?).await?;
        Ok((load.running, load.capacity))
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
