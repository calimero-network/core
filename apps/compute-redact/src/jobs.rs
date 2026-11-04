//! A redaction job as state keeps it, and how its runs' outcomes move it along. The preview run
//! reports the row count, which plans the parts; each part's output is kept by `BlobRef`.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use calimero_sdk::compute::{ComputeOutcome, ComputeStatus};
use calimero_sdk::BlobRef;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::rules::{Policy, Preview};

/// The most parts a job is split into; a larger file gets larger parts.
pub const MAX_PARTS: u64 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Previewing,
    Queued,
    Running,
    Done,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Part {
    pub start: u32,
    pub rows: u32,
    pub status: Status,
    pub cells: u64,
    pub output: Option<BlobRef>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub submitter: String,
    pub file: BlobRef,
    /// The policy as submitted, so a later `set_policy` does not change a running job.
    pub policy: Policy,
    pub policy_hash: String,
    pub rows_per_part: u32,
    pub status: Status,
    pub total_rows: u64,
    pub preview: Option<Preview>,
    pub parts: Vec<Part>,
    pub reason: Option<String>,
}

/// What `job` answers: the job without its policy, which holds the salt.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobView {
    pub id: String,
    pub submitter: String,
    pub status: Status,
    pub policy_hash: String,
    pub total_rows: u64,
    pub rows_done: u64,
    pub cells_redacted: u64,
    pub parts: Vec<PartView>,
    pub preview: Option<Preview>,
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PartView {
    pub status: Status,
    pub rows: u32,
    pub output: Option<BlobRef>,
}

/// `(start, rows)` of each part of a file of `total` rows: `per_part` rows each, raised so there are at most `MAX_PARTS`.
pub fn plan_parts(total: u64, per_part: u32) -> Vec<(u32, u32)> {
    let per_part = u64::from(per_part).max(total.div_ceil(MAX_PARTS)).max(1);
    (0..total)
        .step_by(usize::try_from(per_part).unwrap_or(usize::MAX))
        .map(|start| (start as u32, per_part.min(total - start) as u32))
        .collect()
}

/// The SHA-256 of the policy's canonical JSON, hex.
pub fn policy_hash(policy: &Policy) -> String {
    let json = serde_json::to_vec(policy).expect("a policy is JSON");
    Sha256::digest(json)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The typed result of a done run, or why it did not finish.
pub fn decode<T: DeserializeOwned>(outcome: &ComputeOutcome) -> Result<T, String> {
    if outcome.status != ComputeStatus::Done {
        return Err(outcome
            .reason
            .clone()
            .unwrap_or_else(|| format!("{:?}", outcome.status)));
    }
    let result = outcome
        .result
        .as_deref()
        .ok_or("the run returned no result")?;
    let bytes = STANDARD.decode(result).map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

impl Job {
    pub fn new(submitter: String, file: BlobRef, policy: Policy, rows_per_part: u32) -> Self {
        Self {
            submitter,
            file,
            policy_hash: policy_hash(&policy),
            policy,
            rows_per_part,
            status: Status::Previewing,
            total_rows: 0,
            preview: None,
            parts: Vec::new(),
            reason: None,
        }
    }

    /// The preview ran: plan the parts, all queued; a file with no rows is done.
    pub fn previewed(&mut self, preview: Preview) {
        self.total_rows = preview.total_rows;
        self.preview = Some(preview);
        self.parts = plan_parts(self.total_rows, self.rows_per_part)
            .into_iter()
            .map(|(start, rows)| Part {
                start,
                rows,
                status: Status::Queued,
                cells: 0,
                output: None,
            })
            .collect();
        self.status = if self.parts.is_empty() {
            Status::Done
        } else {
            Status::Running
        };
    }

    pub fn part_started(&mut self, index: usize) {
        self.parts[index].status = Status::Running;
    }

    /// Records a part's output; false when the job already failed, so the output is not kept.
    pub fn part_done(&mut self, index: usize, output: BlobRef, cells: u64) -> bool {
        if self.status == Status::Failed {
            return false;
        }
        let part = &mut self.parts[index];
        part.status = Status::Done;
        part.cells = cells;
        part.output = Some(output);
        if self.parts.iter().all(|p| p.status == Status::Done) {
            self.status = Status::Done;
        }
        true
    }

    /// Fails the job and the parts not yet done.
    pub fn fail(&mut self, reason: String) {
        self.status = Status::Failed;
        self.reason = Some(reason);
        for part in self.parts.iter_mut().filter(|p| p.status != Status::Done) {
            part.status = Status::Failed;
        }
    }

    pub fn view(&self, id: &str) -> JobView {
        let done = self.parts.iter().filter(|p| p.status == Status::Done);
        JobView {
            id: id.to_owned(),
            submitter: self.submitter.clone(),
            status: self.status,
            policy_hash: self.policy_hash.clone(),
            total_rows: self.total_rows,
            rows_done: done.clone().map(|p| u64::from(p.rows)).sum(),
            cells_redacted: done.map(|p| p.cells).sum(),
            parts: self
                .parts
                .iter()
                .map(|p| PartView {
                    status: p.status,
                    rows: p.rows,
                    output: p.output,
                })
                .collect(),
            preview: self.preview.clone(),
            reason: self.reason.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> Policy {
        Policy {
            salt: "pepper".to_owned(),
            columns: [("name".to_owned(), crate::rules::Rule::Pseudonym)].into(),
        }
    }

    fn blob(n: u8) -> BlobRef {
        BlobRef::parse(&format!("{n:02x}").repeat(32)).unwrap()
    }

    fn preview(total_rows: u64) -> Preview {
        Preview {
            header: vec!["name".to_owned()],
            rows: vec![],
            total_rows,
            cells: 0,
        }
    }

    fn running(total: u64, per_part: u32) -> Job {
        let mut job = Job::new("ann".to_owned(), blob(1), policy(), per_part);
        job.previewed(preview(total));
        job
    }

    fn outcome(json: serde_json::Value) -> ComputeOutcome {
        let mut base = serde_json::json!({
            "run": "r1", "method": "preview", "attempt": 1, "status": "done", "reason": null,
            "result": null, "outputs": [],
            "usage": {"cpuSeconds": 0.0, "gibSeconds": 0.0, "fuel": "0"},
        });
        base.as_object_mut()
            .unwrap()
            .extend(json.as_object().unwrap().clone());
        serde_json::from_value(base).unwrap()
    }

    #[test]
    fn parts_cover_the_rows_once() {
        assert_eq!(plan_parts(10, 4), [(0, 4), (4, 4), (8, 2)]);
        assert_eq!(plan_parts(0, 4), []);
        assert_eq!(plan_parts(8, 4), [(0, 4), (4, 4)]);
    }

    #[test]
    fn a_big_file_gets_larger_parts_not_more_than_the_cap() {
        let parts = plan_parts(1_000_000, 1);
        assert!(parts.len() as u64 <= MAX_PARTS);
        assert_eq!(parts.iter().map(|p| u64::from(p.1)).sum::<u64>(), 1_000_000);
    }

    #[test]
    fn previewing_plans_the_parts() {
        let job = running(10, 4);
        assert_eq!(job.status, Status::Running);
        assert_eq!(job.total_rows, 10);
        assert_eq!(job.parts.len(), 3);
        assert!(job.parts.iter().all(|p| p.status == Status::Queued));
        let empty = running(0, 4);
        assert_eq!(empty.status, Status::Done);
    }

    #[test]
    fn the_job_is_done_when_every_part_is() {
        let mut job = running(10, 4);
        assert!(job.part_done(0, blob(2), 5));
        assert!(job.part_done(2, blob(4), 1));
        assert_eq!(job.status, Status::Running);
        assert!(job.part_done(1, blob(3), 2));
        assert_eq!(job.status, Status::Done);
        let view = job.view("j");
        assert_eq!((view.rows_done, view.cells_redacted), (10, 8));
        assert_eq!(view.parts[1].output, Some(blob(3)));
    }

    #[test]
    fn a_failure_fails_the_job_and_late_outputs_are_not_kept() {
        let mut job = running(10, 4);
        assert!(job.part_done(0, blob(2), 5));
        job.fail("fuel".to_owned());
        assert_eq!(job.status, Status::Failed);
        assert_eq!(job.reason.as_deref(), Some("fuel"));
        assert_eq!(job.parts[0].status, Status::Done);
        assert_eq!(job.parts[1].status, Status::Failed);
        assert!(!job.part_done(1, blob(3), 1));
        assert_eq!(job.parts[1].output, None);
    }

    #[test]
    fn the_view_holds_no_salt_and_the_hash_follows_the_policy() {
        let job = running(3, 4);
        let text = serde_json::to_string(&job.view("j")).unwrap();
        assert!(!text.contains("pepper"));
        assert_eq!(job.policy_hash.len(), 64);
        let mut other = policy();
        other.salt = "salt".to_owned();
        assert_ne!(policy_hash(&other), job.policy_hash);
        assert_eq!(policy_hash(&policy()), job.policy_hash);
    }

    #[test]
    fn decode_reads_a_done_runs_result_and_names_a_failure() {
        let body = STANDARD.encode(r#"{"rows":3,"cells":2}"#);
        #[derive(Deserialize)]
        struct R {
            rows: u64,
            cells: u64,
        }
        let r: R = decode(&outcome(serde_json::json!({ "result": body }))).unwrap();
        assert_eq!((r.rows, r.cells), (3, 2));
        let failed = outcome(serde_json::json!({ "status": "failed", "reason": "fuel" }));
        assert_eq!(decode::<R>(&failed).err().as_deref(), Some("fuel"));
        let none = outcome(serde_json::json!({}));
        assert!(decode::<R>(&none).is_err());
    }
}
