//! Ending a run from a job node's signed report: checking it, fetching and storing the outputs,
//! and the arguments the result method receives.

use std::time::Instant;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use calimero_log::Bytes;
use serde_json::{json, Value};
use tracing::warn;

use super::{stored_of, Blob, Drive, FinalPlan, Stored, Watch};
use crate::app::AppMachine;
use crate::component::{Attempt, FailedReason, FileMark, FinalOutcome, RunRecord, RunState};
use crate::files::{
    file_id, sealed_len, segments_match, wrap_key, FileOpener, CHUNK_BYTES, SEGMENT_BYTES,
};
use crate::job_wire::{hash_bytes, FailReason, Report, ReportStatus};
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
                Staging::Ready(outputs) => FinalPlan::reported(report, signature, outputs),
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
            FinalPlan::reported(report, signature, Vec::new())
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
            return Staging::Ready(marks.clone());
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
            let step = self.store_output(&peer, attempt.attempt, report, i).await;
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
                    return staging;
                }
            }
        }
        watch.staged = Some(marks.clone());
        Staging::Ready(marks)
    }

    /// Output `i` of `report`: fetched, checked against the hash and the size the job signed,
    /// opened with its key, stored on the replicas and described as a sealed file for the final to register.
    async fn store_output(
        &self,
        peer: &JobPeer,
        attempt: u32,
        report: &Report,
        i: usize,
    ) -> Result<FileMark, Staging> {
        let out = &report.outputs[i];
        let len = sealed_len(out.size);
        let Ok(plain_hash) = hash_bytes(&out.sha256) else {
            return Err(Staging::Lost);
        };
        let (ciphertext, key) = self
            .fetch_output(peer, attempt, i, len)
            .await
            .map_err(|e| {
                if busy_here(&e) {
                    Staging::Wait
                } else {
                    Staging::Retry
                }
            })?;
        let (size, id) = (out.size, out.id.clone());
        let mark_segments = out.segments.clone();
        let opened = tokio::task::spawn_blocking(move || {
            let hashes: Vec<[u8; 32]> = (mark_segments.iter())
                .map(|s| hash_bytes(s).ok())
                .collect::<Option<_>>()?;
            let intact = ciphertext.len() as u64 == len
                && hex::encode(file_id(&hashes)) == id
                && segments_match(&ciphertext, &hashes);
            if !intact {
                return None;
            }
            let mut opener = FileOpener::new(&key);
            for chunk in ciphertext.chunks(CHUNK_BYTES) {
                opener.push(chunk).ok()?;
            }
            opener.finish_expecting(size, &plain_hash).ok()?;
            Some((ciphertext, key))
        })
        .await
        .map_err(|_| Staging::Retry)?;
        let Some((ciphertext, key)) = opened else {
            return Err(Staging::Lost);
        };
        let file_key = crate::backup::file_key(&self.node.cluster_key, &self.app.app_id);
        let wrapped = wrap_key(&file_key, &self.app.app_id, &key).map_err(|_| Staging::Lost)?;
        let blobs: Vec<Blob> = (ciphertext.chunks(SEGMENT_BYTES).zip(&out.segments))
            .map(|(bytes, hash)| Blob {
                id: hash.clone(),
                bytes: axum::body::Bytes::copy_from_slice(bytes),
                segment: true,
            })
            .collect();
        self.node
            .push_files(&self.app.app_id, &blobs, true)
            .await
            .map_err(|_| Staging::Wait)?;
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
