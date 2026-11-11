//! Ending a run from a job node's signed report: checking it, fetching and storing the outputs,
//! and the arguments the result method receives.

use std::time::Instant;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use calimero_log::Bytes;
use serde_json::{json, Value};
use tracing::warn;

use super::job_io::Held;
use super::{stored_of, Blob, Drive, FinalPlan, Stored, Watch};
use crate::app::AppMachine;
use crate::component::{Attempt, FailedReason, FileMark, FinalOutcome, RunRecord, RunState};
use crate::files::{file_id, sealed_len, wrap_key, SegmentReader, SEGMENT_BYTES};
use crate::job_wire::{hash_bytes, FailReason, OutputRef, Report, ReportStatus, MAX_RUN_LOG_BYTES};
use crate::node::job::busy_here;
use crate::node::{JobPeer, QUOTA_EXCEEDED, REJECTED};

/// What storing a report's outputs came to.
enum Staging {
    Ready(Vec<FileMark>),
    /// The job node could not hand the outputs over: it counts as silence.
    Retry,
    /// The charge or the replicas failed on the leader's side: try again, the job node is not at fault.
    Wait,
    /// What the job node handed over is not what it signed: the attempt is lost.
    Lost,
    /// The outputs would pass the payer's quota.
    Quota,
}

/// Whether a failed charge may still have been applied: a refusal by `apply` changed nothing,
/// while a write that stalled or lost its answer may have committed.
fn charge_may_have_landed(error: &crate::error::ApiError) -> bool {
    error.code != REJECTED && error.code != QUOTA_EXCEEDED
}

/// The arguments of the result method: the run's outcome as JSON, 64-bit integers as decimal
/// strings.
pub(super) fn result_args(rec: &RunRecord, plan: &FinalPlan) -> Vec<u8> {
    let status = plan.outcome.state();
    let reason = match plan.outcome {
        FinalOutcome::Failed { reason } if reason != FailedReason::Cancelled => {
            serde_json::to_value(reason).unwrap_or(Value::Null)
        }
        _ => Value::Null,
    };
    let result = match (&plan.report, status) {
        (Some(report), RunState::Done) => Some(STANDARD.encode(&report.result)),
        _ => None,
    };
    let outputs: Vec<Value> = plan
        .outputs
        .iter()
        .map(|o| {
            json!({
                "id": o.id,
                "size": o.size.to_string(),
                "sha256": hex::encode(&o.sha256.0),
            })
        })
        .collect();
    let usage = json!({
        "cpuSeconds": plan.usage.cpu_micros as f64 / 1e6,
        "gibSeconds": plan.usage.gib_seconds(),
        "fuel": plan.usage.fuel.to_string(),
    });
    json!({
        "run": rec.run,
        "method": rec.method,
        "attempt": plan.attempt,
        "status": status,
        "reason": reason,
        "result": result,
        "outputs": outputs,
        "usage": usage,
    })
    .to_string()
    .into_bytes()
}

impl Drive<'_> {
    /// A job node reported `attempt` ended: check the report, store its outputs, end the run.
    /// True when the job node failed to hand the outputs over, which counts as silence.
    pub(super) async fn finish(
        &self,
        attempt: &Attempt,
        report: Report,
        signature: String,
        watch: &mut Watch,
    ) -> bool {
        watch.leader_wait = false;
        let started = Instant::now();
        let acceptable = self.report_is_valid(attempt, &report, &signature)
            && AppMachine::check_usage(&self.rec.caps, &report.usage).is_ok()
            && (report.reason != Some(FailReason::Cancelled) || self.rec.cancel_requested);
        if !acceptable {
            warn!(app = %self.app.app_id, run = %self.rec.run, "a report did not hold up; the attempt is lost");
            self.lost(attempt, watch).await;
            return false;
        }
        watch.held = Some((attempt.attempt, report.clone(), signature.clone()));
        let plan = if report.status == ReportStatus::Done {
            match self.stage(attempt, &report, watch).await {
                Staging::Ready(outputs) => {
                    let log = self.stage_log(attempt, &report, &outputs, watch).await;
                    FinalPlan::reported(report, signature, outputs, log)
                }
                Staging::Retry => {
                    watch.credit(started.elapsed());
                    return true;
                }
                Staging::Wait => {
                    watch.leader_wait = true;
                    watch.saw();
                    return false;
                }
                Staging::Lost => {
                    self.lost(attempt, watch).await;
                    return false;
                }
                Staging::Quota => FinalPlan::failed(self.rec, FailedReason::Quota),
            }
        } else {
            let log = self.stage_log(attempt, &report, &[], watch).await;
            FinalPlan::reported(report, signature, Vec::new(), log)
        };
        if self.end(&plan, watch).await {
            self.lost(attempt, watch).await;
        }
        false
    }

    /// Charges the outputs, then fetches, checks and stores them one at a time, so only one
    /// file's bytes are held. Kept in `watch` until the final commits.
    async fn stage(&self, attempt: &Attempt, report: &Report, watch: &mut Watch) -> Staging {
        if let Some(marks) = &watch.staged {
            // The log, if one was staged after the outputs, is stage_log's.
            return Staging::Ready(marks[..report.outputs.len().min(marks.len())].to_vec());
        }
        let sizes: Vec<u64> = report.outputs.iter().map(|o| o.size).collect();
        if !AppMachine::outputs_fit(&sizes) {
            return Staging::Lost;
        }
        let (node, app_id) = (self.node, &self.app.app_id);
        let total: u64 = sizes.iter().sum();
        if total > 0 {
            match i64::try_from(total) {
                Ok(delta) => {
                    if let Err(e) = node.count_file_bytes(app_id, delta).await {
                        if e.code == QUOTA_EXCEEDED {
                            return Staging::Quota;
                        }
                        if charge_may_have_landed(&e) {
                            node.refund_files(app_id, total).await;
                        }
                        return Staging::Wait;
                    }
                }
                Err(_) => return Staging::Lost,
            }
        }
        let Ok(peer) = node.attest_job(attempt.node, self.app).await else {
            node.discard(
                &self.app.app_id,
                Stored {
                    charged: total,
                    outputs: true,
                    ..Stored::default()
                },
            );
            return Staging::Retry;
        };
        let mut marks: Vec<FileMark> = Vec::new();
        for i in 0..report.outputs.len() {
            let step = self
                .store_file(
                    &peer,
                    attempt.attempt,
                    &report.outputs[i],
                    Held::Output(i),
                    watch,
                )
                .await;
            match step {
                Ok(mark) => marks.push(mark),
                Err(staging) => {
                    node.discard(
                        app_id,
                        Stored {
                            charged: total,
                            ..stored_of(&marks)
                        },
                    );
                    // The finished outputs were undone; the stored segments of this one stay.
                    watch.fetched.retain(|n, _| *n == i);
                    return staging;
                }
            }
        }
        watch.staged = Some(marks.clone());
        Staging::Ready(marks)
    }

    /// The log of `report`, charged and stored like an output after the `outputs` and kept in
    /// `watch.staged` with them. A log that cannot be had is left out: it never ends or retries the run.
    async fn stage_log(
        &self,
        attempt: &Attempt,
        report: &Report,
        outputs: &[FileMark],
        watch: &mut Watch,
    ) -> Option<FileMark> {
        let log = report.log.as_ref()?;
        if let Some(staged) = watch.staged.as_ref().filter(|m| m.len() > outputs.len()) {
            return staged.last().cloned();
        }
        let (node, app_id) = (self.node, &self.app.app_id);
        let skipped = |why: &str| {
            warn!(app = %app_id, run = %self.rec.run, %why, "a run's log was left out");
        };
        let taken = self.g.read_local(|m, _| super::holds_file(m, &log.id))
            || report.outputs.iter().any(|o| o.id == log.id);
        if log.size > MAX_RUN_LOG_BYTES as u64 || taken {
            skipped("it is over the cap or already a file of the app");
            return None;
        }
        let charge = i64::try_from(log.size).ok()?;
        if let Err(e) = node.count_file_bytes(app_id, charge).await {
            if charge_may_have_landed(&e) {
                node.refund_files(app_id, log.size).await;
            }
            skipped("it cannot be charged");
            return None;
        }
        let refund = Stored {
            charged: log.size,
            ..Stored::default()
        };
        let Ok(peer) = node.attest_job(attempt.node, self.app).await else {
            node.discard(app_id, refund);
            skipped("its job node cannot be attested");
            return None;
        };
        let stored = self
            .store_file(&peer, attempt.attempt, log, Held::Log, watch)
            .await;
        let Ok(mark) = stored else {
            // Its pushed segments go too, once no file of the app lists them.
            let pushed = Stored {
                segments: log.segments.clone(),
                outputs: true,
                ..refund
            };
            node.discard(app_id, pushed);
            skipped("it was not handed over as signed");
            return None;
        };
        let mut staged = outputs.to_vec();
        staged.push(mark.clone());
        watch.staged = Some(staged);
        Some(mark)
    }

    /// `out` of the attempt, fetched segment by segment: each checked against the signed list and
    /// stored on the replicas, then the whole opened to prove size and hash; a cut fetch resumes.
    async fn store_file(
        &self,
        peer: &JobPeer,
        attempt: u32,
        out: &OutputRef,
        held: Held,
        watch: &mut Watch,
    ) -> Result<FileMark, Staging> {
        let i = held.slot();
        let Ok(plain_hash) = hash_bytes(&out.sha256) else {
            return Err(Staging::Lost);
        };
        let hashes: Option<Vec<[u8; 32]>> =
            out.segments.iter().map(|s| hash_bytes(s).ok()).collect();
        let Some(hashes) = hashes.filter(|h| {
            hex::encode(file_id(h)) == out.id
                && h.len() as u64 == sealed_len(out.size).div_ceil(SEGMENT_BYTES as u64)
        }) else {
            return Err(Staging::Lost);
        };
        let cut = |e: eyre::Report| {
            if busy_here(&e) {
                Staging::Wait
            } else {
                Staging::Retry
            }
        };
        let key = self.fetch_key(peer, attempt, held).await.map_err(cut)?;
        let mut reader = SegmentReader::new(&key, hashes.clone());
        let mut next = watch.fetched.get(&i).copied().unwrap_or(0);
        for k in 0..next {
            let held = self.node.local_segment(&out.segments[k]);
            if held.is_none_or(|bytes| reader.push(&bytes).is_err()) {
                // A stored segment went missing: start this output over.
                reader = SegmentReader::new(&key, hashes);
                next = 0;
                break;
            }
        }
        for k in next..out.segments.len() {
            let bytes = self
                .fetch_segment(peer, attempt, held, k)
                .await
                .map_err(cut)?;
            watch.saw();
            if reader.push(&bytes).is_err() {
                return Err(Staging::Lost);
            }
            let blob = Blob {
                id: out.segments[k].clone(),
                bytes: Some(bytes.into()),
                segment: true,
            };
            self.node
                .push_files(&self.app.app_id, &[blob], true)
                .await
                .map_err(|_| Staging::Wait)?;
            watch.fetched.insert(i, k + 1);
        }
        if reader.finish_expecting(out.size, &plain_hash).is_err() {
            return Err(Staging::Lost);
        }
        let file_key = crate::backup::file_key(&self.node.cluster_key, &self.app.app_id);
        let wrapped = wrap_key(&file_key, &self.app.app_id, &key).map_err(|_| Staging::Lost)?;
        Ok(FileMark {
            id: out.id.clone(),
            size: out.size,
            sha256: Bytes(plain_hash.to_vec()),
            wrapped_key: Bytes(wrapped),
            segments: out.segments.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ApiError;

    #[test]
    fn only_a_charge_that_may_have_committed_is_refunded() {
        let rejected = ApiError::bad_request(REJECTED, "no such app");
        let quota = ApiError::new(429, QUOTA_EXCEEDED, "quota exceeded");
        let stalled = ApiError::unavailable("the system group did not answer");
        assert!(!charge_may_have_landed(&rejected));
        assert!(!charge_may_have_landed(&quota));
        assert!(charge_may_have_landed(&stalled));
    }
}
