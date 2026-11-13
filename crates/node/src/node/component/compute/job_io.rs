//! The leader's conversation with a job node: choosing one, building and sending an attempt's
//! assignment, and polling it.

use std::collections::HashMap;

use calimero_log::NodeId;
use futures_util::future::join_all;
use reqwest::Method;
use serde::Deserialize;
use tracing::debug;
use zeroize::Zeroizing;

use super::{run_path, Drive};
use crate::component::blobs::Blobs;
use crate::component::{Attempt, RunRecord};
use crate::files::{
    noise_seed, run_token, unwrap_key, wrap_for_job_derived, JobWrap, WrapKind, SEGMENT_BYTES,
};
use crate::job_node::{LOG_HEADER, NEED_BUNDLE, TOKEN_HEADER, UNKNOWN_RUN};
use crate::job_wire::{
    assignment_digest, hash_bytes, verify_report, Assignment, Report, RunBudget, RunInput,
    BUNDLE_SIGNATURE_HEADER,
};
use crate::node::job::{
    busy_here, timed_out, AnswerTooLarge, JobAnswer, JOB_DELETE_TIMEOUT, MAX_JOB_STATUS_BYTES,
};
use crate::node::{JobPeer, Node};
use crate::types::AppRecord;

#[derive(Deserialize)]
struct OutputKey {
    key: String,
}

/// A sealed file of an attempt a job node hands over: one of its outputs, or its log.
#[derive(Clone, Copy)]
pub(super) enum Held {
    Output(usize),
    Log,
}

impl Held {
    /// The file's path under the attempt's.
    fn path(self) -> String {
        match self {
            Self::Output(i) => format!("outputs/{i}"),
            Self::Log => "log".to_owned(),
        }
    }

    /// Where `Watch::fetched` counts the segments stored of the file.
    pub(super) fn slot(self) -> usize {
        match self {
            Self::Output(i) => i,
            Self::Log => usize::MAX,
        }
    }
}

/// What a job node says about an attempt.
#[derive(Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum JobStatus {
    Awaiting,
    Running {
        #[serde(default)]
        progress: Option<Progress>,
    },
    Done {
        report: Box<Report>,
        signature: String,
    },
    Lost,
}

#[derive(Deserialize)]
pub struct Progress {
    percent: u8,
    step: String,
}

/// The result of a poll.
pub enum Polled {
    /// This node's own view of the directory could not be brought up to date: nothing is known.
    Deferred,
    /// No usable answer.
    Silent,
    /// The job node does not hold the attempt.
    Unknown,
    /// It holds it, still waiting for its inputs.
    Awaiting,
    Running(Option<(u8, String)>),
    Done(Box<Report>, String),
    /// The job node says it cannot serve the attempt, or is no longer the node that was chosen.
    Lost,
}

/// What a job node said when its run was deleted.
pub enum Deleted {
    /// No such run, or the node is unreachable.
    Gone,
    /// The run did not stop in time, or the delete did not answer in time: ask again.
    Running,
    /// The run ended: its last status was a signed report.
    Ended(Box<Report>, String),
}

/// The result of sending an attempt.
pub enum Sent {
    Delivered,
    /// Try again next round.
    Failed,
    /// The assignment can never be the one that was committed: the attempt is lost.
    Impossible,
}

impl Node {
    /// Each job node not in `skip` that attests and answers its load, with its running count and
    /// capacity. Holds no lock: a node that never answers costs only its own timeout.
    pub(super) async fn survey_jobs(
        &self,
        app: &AppRecord,
        skip: &[NodeId],
    ) -> Vec<(usize, usize, JobPeer)> {
        let ids: Vec<NodeId> = self.sys(|s| {
            s.job_nodes()
                .map(|n| n.id)
                .filter(|id| !skip.contains(id))
                .collect()
        });
        let found = join_all(ids.into_iter().map(|id| async move {
            let peer = self.attest_job(id, app).await.ok()?;
            let (running, capacity) = self.job_load(&peer).await.ok()?;
            Some((running, capacity, peer))
        }))
        .await;
        found.into_iter().flatten().collect()
    }

    /// The assignment of `attempt` to `peer`, signed by the cluster identity: a function of the
    /// record, the blob records and the job node's keys alone, so a new leader builds the same bytes.
    pub(super) fn build_assignment(
        &self,
        app: &AppRecord,
        rec: &RunRecord,
        attempt: u32,
        peer: &JobPeer,
        blobs: &Blobs,
    ) -> eyre::Result<Assignment> {
        let file_key = crate::backup::file_key(&self.cluster_key, &app.app_id);
        let wrap = |kind, index| JobWrap {
            app_id: &app.app_id,
            run_id: &rec.run,
            attempt,
            kind,
            index,
        };
        let mut inputs = Vec::with_capacity(rec.inputs.len());
        for (i, input) in rec.inputs.iter().enumerate() {
            let seal = <[u8; 32]>::try_from(hex::decode(&input.id)?)
                .ok()
                .and_then(|id| blobs.get(&id))
                .and_then(|record| record.file.as_ref())
                .ok_or_else(|| eyre::eyre!("input {} is no longer a file", input.id))?;
            let data_key = unwrap_key(&file_key, &app.app_id, &seal.wrapped_key.0)?;
            let index = u32::try_from(i)?;
            inputs.push(RunInput {
                id: input.id.clone(),
                size: input.size,
                sha256: input.sha256.clone(),
                segments: seal.segments.clone(),
                wrapped: wrap_for_job_derived(
                    &file_key,
                    &peer.transport_key,
                    &wrap(WrapKind::FileKey, index),
                    &data_key,
                )?,
            });
        }
        let bundle = rec
            .compute_bundle
            .clone()
            .ok_or_else(|| eyre::eyre!("the run has no compute component"))?;
        let mut assignment = Assignment {
            app: app.app_id.clone(),
            run: rec.run.clone(),
            attempt,
            method: rec.method.clone(),
            args: rec.args.0.clone(),
            bundle,
            caller: rec.caller.clone(),
            owner: rec.owner.clone(),
            start_ts_ms: rec.start_ts_ms,
            budget: RunBudget {
                fuel: rec.caps.fuel,
                memory_mib: rec.caps.memory_mib,
                wall_secs: rec.caps.wall_secs,
            },
            inputs,
            seed: wrap_for_job_derived(
                &file_key,
                &peer.transport_key,
                &wrap(WrapKind::NoiseSeed, 0),
                &noise_seed(&file_key, &rec.run),
            )?,
            token: hex::encode(run_token(&file_key, &rec.run, attempt)),
            signature: String::new(),
        };
        assignment.signature = self.sign_assignment(&assignment)?;
        Ok(assignment)
    }
}

impl Drive<'_> {
    fn token(&self, attempt: u32) -> String {
        let file_key = crate::backup::file_key(&self.node.cluster_key, &self.app.app_id);
        hex::encode(run_token(&file_key, &self.rec.run, attempt))
    }

    fn path(&self, attempt: u32) -> String {
        format!(
            "job/runs/{}",
            run_path(&self.app.app_id, &self.rec.run, attempt)
        )
    }

    /// Asks the job node about `attempt`.
    pub(super) async fn poll(&self, attempt: &Attempt) -> Polled {
        if !self.node.sys_fresh().await {
            return Polled::Deferred;
        }
        let Ok(peer) = self.node.attest_job(attempt.node, self.app).await else {
            return Polled::Silent;
        };
        // A restarted job node has new keys and holds no run.
        if hex::encode(peer.job_key) != attempt.job_key {
            return Polled::Lost;
        }
        let token = self.token(attempt.attempt);
        let headers = self.run_headers(&token).await;
        let answer = self
            .node
            .job_call(
                &peer,
                Method::GET,
                &self.path(attempt.attempt),
                &headers,
                Vec::new(),
                MAX_JOB_STATUS_BYTES,
            )
            .await;
        let answer = match answer {
            Ok(answer) => answer,
            // Our own queue was full: nothing is known of the job node.
            Err(e) if busy_here(&e) => return Polled::Deferred,
            // A status the leader will not read is not silence: the job node is misbehaving.
            Err(e) if e.downcast_ref::<AnswerTooLarge>().is_some() => return Polled::Lost,
            Err(_) => return Polled::Silent,
        };
        if answer.status == 404 && answer.code().as_deref() == Some(UNKNOWN_RUN) {
            return Polled::Unknown;
        }
        if answer.status != 200 {
            return Polled::Silent;
        }
        match answer.json::<JobStatus>() {
            Ok(JobStatus::Awaiting) => Polled::Awaiting,
            Ok(JobStatus::Running { progress }) => {
                Polled::Running(progress.map(|p| (p.percent.min(100), p.step)))
            }
            Ok(JobStatus::Done { report, signature }) => Polled::Done(report, signature),
            Ok(JobStatus::Lost) | Err(_) => Polled::Lost,
        }
    }

    /// Sends the recorded `attempt`: the component if asked for, the assignment, then every input
    /// still awaited. A job node answers an assignment it holds as it stands, so this repeats safely.
    pub(super) async fn send(&self, attempt: &Attempt) -> Sent {
        let node = self.node;
        let Ok(peer) = node.attest_job(attempt.node, self.app).await else {
            return Sent::Failed;
        };
        if hex::encode(peer.job_key) != attempt.job_key {
            return Sent::Impossible;
        }
        let blobs = self.g.read_local(|m, _| m.blob_view());
        let Ok(assignment) =
            node.build_assignment(self.app, self.rec, attempt.attempt, &peer, &blobs)
        else {
            return Sent::Impossible;
        };
        match assignment_digest(&assignment) {
            Ok(digest) if hex::encode(digest) == attempt.assignment_digest => {}
            _ => return Sent::Impossible,
        }
        match self.deliver(&peer, &assignment).await {
            Ok(()) => Sent::Delivered,
            Err(e) => {
                debug!(app = %self.app.app_id, run = %self.rec.run, error = %e, "attempt not sent");
                Sent::Failed
            }
        }
    }

    pub(super) async fn deliver(
        &self,
        peer: &JobPeer,
        assignment: &Assignment,
    ) -> eyre::Result<()> {
        let body = serde_json::to_vec(assignment)?;
        let mut answer = self.assign_call(peer, &body).await?;
        if answer.status == 409 && answer.code().as_deref() == Some(NEED_BUNDLE) {
            self.push_bundle(peer).await?;
            answer = self.assign_call(peer, &body).await?;
        }
        if answer.status != 200 {
            eyre::bail!("the job node refused the assignment with {}", answer.status);
        }
        if matches!(answer.json::<JobStatus>()?, JobStatus::Awaiting) {
            self.push_inputs(peer, assignment).await?;
        }
        Ok(())
    }

    /// The headers of a request about a run: its token, and the log header only while every
    /// replica reads a final entry that carries a log. The job node answers with a log only then.
    async fn run_headers<'a>(&self, token: &'a str) -> Vec<(&'static str, &'a str)> {
        let mut headers = vec![(TOKEN_HEADER, token)];
        if self.node.replicas_take_logs().await {
            headers.push((LOG_HEADER, "1"));
        }
        headers
    }

    async fn assign_call(&self, peer: &JobPeer, body: &[u8]) -> eyre::Result<JobAnswer> {
        let logging = self.node.replicas_take_logs().await;
        let headers: &[(&str, &str)] = if logging { &[(LOG_HEADER, "1")] } else { &[] };
        self.node
            .job_transfer(
                peer,
                Method::POST,
                "job/runs",
                headers,
                body.to_vec(),
                MAX_JOB_STATUS_BYTES,
            )
            .await
    }

    async fn push_bundle(&self, peer: &JobPeer) -> eyre::Result<()> {
        let hash = self
            .rec
            .compute_bundle
            .as_ref()
            .ok_or_else(|| eyre::eyre!("the run has no compute component"))?;
        let bytes = self.node.bundle_wasm(hash).await?;
        let signature = self.node.sign_bundle(hash)?;
        let answer = self
            .node
            .job_transfer(
                peer,
                Method::PUT,
                &format!("job/bundles/{hash}"),
                &[(BUNDLE_SIGNATURE_HEADER, &signature)],
                bytes,
                MAX_JOB_STATUS_BYTES,
            )
            .await?;
        if answer.status != 200 {
            eyre::bail!("the job node refused the component with {}", answer.status);
        }
        Ok(())
    }

    /// Sends every segment of every input, one at a time: from this node's store, else fetched
    /// from a replica.
    async fn push_inputs(&self, peer: &JobPeer, assignment: &Assignment) -> eyre::Result<()> {
        let token = self.token(assignment.attempt);
        let replicas = self.node.other_replicas(self.app);
        for (i, input) in assignment.inputs.iter().enumerate() {
            for (k, hash) in input.segments.iter().enumerate() {
                let _slot = self.node.job_slot().await?;
                let segment = self.node.segment_from(hash, &replicas).await?;
                let answer = self
                    .node
                    .job_call_held(
                        peer,
                        Method::PUT,
                        &format!("{}/inputs/{i}/{k}", self.path(assignment.attempt)),
                        &[(TOKEN_HEADER, &token)],
                        segment,
                        MAX_JOB_STATUS_BYTES,
                    )
                    .await?;
                if answer.status != 200 {
                    eyre::bail!(
                        "the job node refused segment {k} of input {i} with {}",
                        answer.status
                    );
                }
            }
        }
        Ok(())
    }

    /// Asks the job node to stop and drop `attempt`: it frees the node's scratch and any run
    /// still going, and answers the run's last status.
    pub(super) async fn delete_job_run(&self, attempt: &Attempt) -> Deleted {
        let Ok(peer) = self.node.attest_job(attempt.node, self.app).await else {
            return Deleted::Gone;
        };
        self.delete_on(&peer, attempt.attempt).await
    }

    pub(super) async fn delete_on(&self, peer: &JobPeer, attempt: u32) -> Deleted {
        let token = self.token(attempt);
        let headers = self.run_headers(&token).await;
        let answer = self
            .node
            .job_call_within(
                JOB_DELETE_TIMEOUT,
                peer,
                Method::DELETE,
                &self.path(attempt),
                &headers,
                Vec::new(),
                MAX_JOB_STATUS_BYTES,
            )
            .await;
        if answer.as_ref().is_err_and(|e| busy_here(e) || timed_out(e)) {
            return Deleted::Running;
        }
        match answer.ok().filter(|a| a.status == 200).map(|a| a.json()) {
            Some(Ok(JobStatus::Running { .. })) => Deleted::Running,
            Some(Ok(JobStatus::Done { report, signature }))
                if self.report_is_valid_for(peer, attempt, &report, &signature) =>
            {
                Deleted::Ended(report, signature)
            }
            _ => Deleted::Gone,
        }
    }

    /// Whether `report` is the signed word of `attempt`'s job node for this run and assignment.
    pub(super) fn report_is_valid(
        &self,
        attempt: &Attempt,
        report: &Report,
        signature: &str,
    ) -> bool {
        let (Ok(key), Ok(digest)) = (
            hash_bytes(&attempt.job_key),
            hash_bytes(&attempt.assignment_digest),
        ) else {
            return false;
        };
        report.app == self.app.app_id
            && report.run == self.rec.run
            && report.attempt == attempt.attempt
            && verify_report(&key, &digest, report, signature)
    }

    /// [`Self::report_is_valid`] for the attempt number `attempt` as recorded, when the job node
    /// is `peer`.
    fn report_is_valid_for(
        &self,
        peer: &JobPeer,
        attempt: u32,
        report: &Report,
        signature: &str,
    ) -> bool {
        self.rec
            .attempt(attempt)
            .is_some_and(|a| a.node == peer.id && self.report_is_valid(a, report, signature))
    }

    /// The data key of `held`, fetched from the job node.
    pub(super) async fn fetch_key(
        &self,
        peer: &JobPeer,
        attempt: u32,
        held: Held,
    ) -> eyre::Result<Zeroizing<[u8; 32]>> {
        let token = self.token(attempt);
        let path = format!("{}/{}/key", self.path(attempt), held.path());
        let key = self
            .node
            .job_call(
                peer,
                Method::GET,
                &path,
                &[(TOKEN_HEADER, token.as_str())],
                Vec::new(),
                MAX_JOB_STATUS_BYTES,
            )
            .await?;
        let key: OutputKey = key.json()?;
        let key: [u8; 32] = Zeroizing::new(hex::decode(key.key)?)
            .as_slice()
            .try_into()
            .map_err(|_| eyre::eyre!("an output key is 32 bytes"))?;
        Ok(Zeroizing::new(key))
    }

    /// Segment `k` of `held`, at most one segment, fetched from the job node.
    pub(super) async fn fetch_segment(
        &self,
        peer: &JobPeer,
        attempt: u32,
        held: Held,
        k: usize,
    ) -> eyre::Result<Vec<u8>> {
        let token = self.token(attempt);
        let path = format!("{}/{}/{k}", self.path(attempt), held.path());
        let answer = self
            .node
            .job_transfer(
                peer,
                Method::GET,
                &path,
                &[(TOKEN_HEADER, token.as_str())],
                Vec::new(),
                SEGMENT_BYTES,
            )
            .await?;
        if answer.status != 200 {
            eyre::bail!(
                "the job node answered {} for segment {k} of {}",
                answer.status,
                held.path()
            );
        }
        Ok(answer.body)
    }
}

/// The free job node of `surveyed` in the run's pool (`in_pool`) with the fewest runs (those
/// being delivered to it counted), lowest id first; one of `avoid` only when no other is free.
pub(super) fn pick_job(
    surveyed: Vec<(usize, usize, JobPeer)>,
    avoid: &[NodeId],
    reserved: &HashMap<NodeId, usize>,
    in_pool: &dyn Fn(NodeId) -> bool,
) -> Option<JobPeer> {
    let mut free: Vec<_> = surveyed
        .into_iter()
        .filter(|(_, _, peer)| in_pool(peer.id))
        .map(|(running, capacity, peer)| {
            (
                running + reserved.get(&peer.id).copied().unwrap_or(0),
                capacity,
                peer,
            )
        })
        .filter(|(running, capacity, _)| running < capacity)
        .map(|(running, _, peer)| (running, peer.id, peer))
        .collect();
    free.sort_by_key(|(running, id, _)| (*running, *id));
    let pick = free
        .iter()
        .position(|(_, id, _)| !avoid.contains(id))
        .or_else(|| (!free.is_empty()).then_some(0))?;
    Some(free.swap_remove(pick).2)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn peer(id: NodeId) -> JobPeer {
        let url = format!("http://127.0.0.1:{}", 1000 + id);
        JobPeer {
            id,
            url: url.clone(),
            transport_key: [0; 32],
            job_key: [0; 32],
            release: String::new(),
            channel: Arc::new(calimero_sealed::client::SealedTransport::with_key(
                url.parse().unwrap(),
                reqwest::Client::new(),
                [0; 32],
            )),
        }
    }

    #[tokio::test]
    async fn pick_job_stays_in_the_slots_pool() {
        let surveyed = || vec![(0, 2, peer(1)), (1, 2, peer(2)), (0, 2, peer(3))];
        let none = HashMap::new();
        let in_pool = |ids: &'static [NodeId]| move |id: NodeId| ids.contains(&id);
        let pick =
            |ids: &'static [NodeId]| pick_job(surveyed(), &[], &none, &in_pool(ids)).map(|p| p.id);
        assert_eq!(pick(&[2, 3]), Some(3));
        assert_eq!(pick(&[2]), Some(2));
        assert_eq!(pick(&[]), None);
        assert_eq!(
            pick_job(surveyed(), &[], &none, &|_| true).map(|p| p.id),
            Some(1)
        );
    }
}
