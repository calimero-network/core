//! What the node keeps of each pipeline step run when it ends, so a receipt can show later which
//! job node ran which assignment and what came out, after the run's own record is gone.

use calimero_client::receipt::{FileRef, JobEvidence, ProofUsage, StepRunProof};
use calimero_log::StateTx;

use super::pipelines::{reason_code, run_id, PipelineRun};
use super::runs::{FinalEntry, FinalOutcome, RunRecord};
use super::{get_json, parse_hex32, put_json};
use crate::app::{rows, AppMachine};
use crate::attest::AttestationDoc;
use crate::job_wire::report_message;

fn outcome_name(outcome: FinalOutcome) -> String {
    match outcome {
        FinalOutcome::Done => "done".to_owned(),
        FinalOutcome::Cancelled => "cancelled".to_owned(),
        FinalOutcome::Failed { reason } => format!("failed:{}", reason_code(reason)),
    }
}

/// SHA-256 of the message the job signed, when the final carries its report.
fn report_hash(record: &RunRecord, entry: &FinalEntry) -> Option<String> {
    let digest = parse_hex32(&record.attempts.last()?.assignment_digest)?;
    let message = report_message(entry.report.as_ref()?, &digest).ok()?;
    Some(hex::encode(calimero_identity::sha256(&message)))
}

impl AppMachine {
    /// The proof of step run `run`, if it ran and its pipeline run is still kept.
    #[must_use]
    pub fn step_proof(&self, run: &str) -> Option<StepRunProof> {
        let raw = self
            .state
            .kv
            .group()?
            .get_hidden(rows::CPROOF, run.as_bytes())?;
        serde_json::from_slice(&raw).ok()
    }

    /// Keeps the proof of a linked run that ended with `entry`, and the attestation of the job
    /// node it names once per pipeline run; other runs have none.
    pub(super) fn write_step_proof(tx: &mut StateTx<'_>, record: &RunRecord, entry: &FinalEntry) {
        let Some(link) = &record.pipeline else {
            return;
        };
        let attempt = record.attempts.last();
        if let Some(a) = attempt {
            if let Some(doc) = a
                .job_attestation
                .as_ref()
                .filter(|d| d.job_key.as_deref() == Some(a.job_key.as_str()))
            {
                put_json(tx, rows::CJOB, &rows::cjob(&link.run, &a.job_key), doc);
            }
        }
        let proof = StepRunProof {
            run: record.run.clone(),
            step: link.step.clone(),
            index: link.index,
            outcome: outcome_name(entry.outcome),
            attempt: entry.attempt,
            node: attempt.map(|a| a.node),
            job_key: attempt.map(|a| a.job_key.clone()),
            assignment_digest: attempt.map(|a| a.assignment_digest.clone()),
            report_sha256: report_hash(record, entry),
            signature: entry.report.as_ref().and(entry.signature.clone()),
            outputs: entry
                .outputs
                .iter()
                .map(|o| FileRef {
                    id: o.id.clone(),
                    size: o.size,
                    sha256: hex::encode(&o.sha256.0),
                })
                .collect(),
            usage: ProofUsage {
                cpu_micros: entry.usage.cpu_micros.to_string(),
                gib_micros: entry.usage.gib_micros.to_string(),
                fuel: entry.usage.fuel.to_string(),
            },
            job: None,
        };
        if let Some(r) = &entry.report {
            let evidence = JobEvidence {
                result_sha256: hex::encode(calimero_identity::sha256(&r.result)),
                message: r.message.clone(),
                segments: r.outputs.iter().map(|o| o.segments.clone()).collect(),
                log: r.log.clone(),
            };
            put_json(tx, rows::CREPORT, &record.run, &evidence);
        }
        put_json(tx, rows::CPROOF, &record.run, &proof);
    }

    /// Drops the proofs of every run `pipeline` started, and the job attestations they name,
    /// with its record.
    pub(super) fn drop_step_proofs(tx: &mut StateTx<'_>, pipeline: &PipelineRun) {
        for (name, step) in &pipeline.steps {
            for index in 0..step.started as usize {
                let id = run_id(&pipeline.id, name, index);
                if let Some(key) =
                    get_json::<StepRunProof>(tx, rows::CPROOF, &id).and_then(|p| p.job_key)
                {
                    tx.delete_hidden(rows::CJOB, rows::cjob(&pipeline.id, &key).as_bytes());
                }
                tx.delete_hidden(rows::CREPORT, id.as_bytes());
                tx.delete_hidden(rows::CPROOF, id.as_bytes());
            }
        }
    }

    /// What step run `run`'s job report signed beyond its proof, if a job reported it.
    #[must_use]
    pub fn job_evidence(&self, run: &str) -> Option<JobEvidence> {
        let raw = self
            .state
            .kv
            .group()?
            .get_hidden(rows::CREPORT, run.as_bytes())?;
        serde_json::from_slice(&raw).ok()
    }

    /// The attestation of job key `key` kept for pipeline run `pipeline`.
    #[must_use]
    pub fn job_attestation(&self, pipeline: &str, key: &str) -> Option<AttestationDoc> {
        let raw = self
            .state
            .kv
            .group()?
            .get_hidden(rows::CJOB, rows::cjob(pipeline, key).as_bytes())?;
        serde_json::from_slice(&raw).ok()
    }
}
