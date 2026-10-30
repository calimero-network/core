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
use crate::component::{Attempt, Marks, RunRecord};
use crate::files::{noise_seed, run_token, unwrap_key, wrap_for_job_derived, JobWrap, WrapKind};
use crate::job_node::{NEED_BUNDLE, TOKEN_HEADER, UNKNOWN_RUN};
use crate::job_wire::{
    assignment_digest, hash_bytes, verify_report, Assignment, Report, RunBudget, RunInput,
    BUNDLE_SIGNATURE_HEADER,
};
use crate::node::job::{busy_here, AnswerTooLarge, JobAnswer, MAX_JOB_STATUS_BYTES};
use crate::node::{JobPeer, Node};
use crate::types::AppRecord;

#[derive(Deserialize)]
struct OutputKey {
    key: String,
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
    /// No answer, or no such run.
    Gone,
    /// The run did not stop in time and still runs.
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
    /// The free job node with the fewest runs (those being delivered to it counted), lowest id
    /// first, never one of `skip`; one of `avoid` only when no other is free.
    pub(super) async fn choose_job(
        &self,
        app: &AppRecord,
        avoid: &[NodeId],
        skip: &[NodeId],
        reserved: &HashMap<NodeId, usize>,
    ) -> Option<JobPeer> {
        let ids: Vec<NodeId> = self.sys(|s| {
            s.job_nodes()
                .map(|n| n.id)
                .filter(|id| !skip.contains(id))
                .collect()
        });
        let found = join_all(ids.into_iter().map(|id| async move {
            let peer = self.attest_job(id, app).await.ok()?;
            let (running, capacity) = self.job_load(&peer).await.ok()?;
            let running = running + reserved.get(&id).copied().unwrap_or(0);
            (running < capacity).then_some((running, id, peer))
        }))
        .await;
        let mut free: Vec<_> = found.into_iter().flatten().collect();
        free.sort_by_key(|(running, id, _)| (*running, *id));
        let pick = free
            .iter()
            .position(|(_, id, _)| !avoid.contains(id))
            .or_else(|| (!free.is_empty()).then_some(0))?;
        Some(free.swap_remove(pick).2)
    }

    /// The assignment of `attempt` of `rec` to `peer`, signed by the cluster identity. It is a
    /// function of the record, the marks and the job node's keys alone, so a new leader builds
    /// the very bytes the first one committed the digest of.
    pub(super) fn build_assignment(
        &self,
        app: &AppRecord,
        rec: &RunRecord,
        attempt: u32,
        peer: &JobPeer,
        marks: &Marks,
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
            let mark = marks
                .get(&input.id)
                .ok_or_else(|| eyre::eyre!("input {} is no longer a file", input.id))?;
            let data_key = unwrap_key(&file_key, &app.app_id, &mark.wrapped_key.0)?;
            let index = u32::try_from(i)?;
            inputs.push(RunInput {
                id: input.id.clone(),
                size: input.size,
                sha256: input.sha256.clone(),
                wrapped: wrap_for_job_derived(
                    &file_key,
                    &peer.transport_key,
                    &wrap(WrapKind::FileKey, index),
                    &data_key,
                )?,
            });
        }
        let mut assignment = Assignment {
            app: app.app_id.clone(),
            run: rec.run.clone(),
            attempt,
            method: rec.method.clone(),
            args: rec.args.0.clone(),
            bundle: rec.bundle.clone(),
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

    /// The ciphertext of file `id`: this node's, else the app's other replicas'.
    async fn input_ciphertext(&self, app: &AppRecord, id: &str) -> eyre::Result<Vec<u8>> {
        if let Ok(Some(bytes)) = self.engine.get_blob(id) {
            if hex::encode(calimero_identity::sha256(&bytes)) == id {
                return Ok(bytes);
            }
        }
        Ok(self.fetch_blob_from(id, &self.other_replicas(app)).await?)
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
        let answer = self
            .node
            .job_call(
                &peer,
                Method::GET,
                &self.path(attempt.attempt),
                &[(TOKEN_HEADER, &token)],
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

    /// Sends the recorded `attempt`: the component if the job node asks for it, the assignment,
    /// then every input if it still waits for them. The job node answers an assignment it holds
    /// as it stands, so this is safe to repeat.
    pub(super) async fn send(&self, attempt: &Attempt) -> Sent {
        let node = self.node;
        let Ok(peer) = node.attest_job(attempt.node, self.app).await else {
            return Sent::Failed;
        };
        if hex::encode(peer.job_key) != attempt.job_key {
            return Sent::Impossible;
        }
        let marks = self.g.read_local(|m, _| m.file_marks());
        let Ok(assignment) =
            node.build_assignment(self.app, self.rec, attempt.attempt, &peer, &marks)
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

    async fn assign_call(&self, peer: &JobPeer, body: &[u8]) -> eyre::Result<JobAnswer> {
        self.node
            .job_transfer(
                peer,
                Method::POST,
                "job/runs",
                &[],
                body.to_vec(),
                MAX_JOB_STATUS_BYTES,
            )
            .await
    }

    async fn push_bundle(&self, peer: &JobPeer) -> eyre::Result<()> {
        let hash = &self.rec.bundle;
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

    async fn push_inputs(&self, peer: &JobPeer, assignment: &Assignment) -> eyre::Result<()> {
        let token = self.token(assignment.attempt);
        for (i, input) in assignment.inputs.iter().enumerate() {
            let _slot = self.node.job_slot().await?;
            let ciphertext = self.node.input_ciphertext(self.app, &input.id).await?;
            let answer = self
                .node
                .job_call_held(
                    peer,
                    Method::PUT,
                    &format!("{}/inputs/{i}", self.path(assignment.attempt)),
                    &[(TOKEN_HEADER, &token)],
                    ciphertext,
                    MAX_JOB_STATUS_BYTES,
                )
                .await?;
            if answer.status != 200 {
                eyre::bail!("the job node refused input {i} with {}", answer.status);
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
        let answer = self
            .node
            .job_call(
                peer,
                Method::DELETE,
                &self.path(attempt),
                &[(TOKEN_HEADER, &token)],
                Vec::new(),
                MAX_JOB_STATUS_BYTES,
            )
            .await;
        if answer.as_ref().is_err_and(busy_here) {
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
            .attempts
            .get(attempt as usize - 1)
            .is_some_and(|a| a.node == peer.id && self.report_is_valid(a, report, signature))
    }

    /// One output's ciphertext and data key, fetched from the job node.
    pub(super) async fn fetch_output(
        &self,
        peer: &JobPeer,
        attempt: u32,
        i: usize,
        sealed_len: u64,
    ) -> eyre::Result<(Vec<u8>, Zeroizing<[u8; 32]>)> {
        let token = self.token(attempt);
        let base = format!("{}/outputs/{i}", self.path(attempt));
        let headers = [(TOKEN_HEADER, token.as_str())];
        let cap = usize::try_from(sealed_len)?;
        let body = self
            .node
            .job_transfer(peer, Method::GET, &base, &headers, Vec::new(), cap)
            .await?;
        if body.status != 200 {
            eyre::bail!("the job node answered {} for output {i}", body.status);
        }
        let key = self
            .node
            .job_call(
                peer,
                Method::GET,
                &format!("{base}/key"),
                &headers,
                Vec::new(),
                MAX_JOB_STATUS_BYTES,
            )
            .await?;
        let key: OutputKey = key.json()?;
        let key: [u8; 32] = Zeroizing::new(hex::decode(key.key)?)
            .as_slice()
            .try_into()
            .map_err(|_| eyre::eyre!("an output key is 32 bytes"))?;
        Ok((body.body, Zeroizing::new(key)))
    }
}
