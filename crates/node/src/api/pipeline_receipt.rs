//! `GET /apps/{app}/pipeline-runs/{run}/receipt`: the node's signed statement of which code ran
//! on which job nodes over which inputs, and which files came out
//! ([`calimero_client::receipt`]).

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use calimero_client::receipt::{
    CachedRun, FileRef, PipelineReceipt, RunEntry, StepReceipt, StepRunProof,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{gate, may_handle_files, ApiResult};
use crate::component::{PipelineRun, PipelineState, RunInputRef, StepState};
use crate::error::ApiError;
use crate::node::{Node, PIPELINE_VIEW};
use crate::packages::pipelines::SELF_MODULE;

const RECEIPT_VERSION: u32 = 1;
const RUN_NOT_ENDED: &str = "RUN_NOT_ENDED";

/// What the reserved view answers for a receipt: the record and the proof of each run it started.
#[derive(Serialize, Deserialize)]
pub struct ReceiptSource {
    pub run: PipelineRun,
    pub proofs: BTreeMap<String, StepRunProof>,
}

/// The name a unit enum serializes to.
fn name_of<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn file_ref(f: &RunInputRef) -> FileRef {
    FileRef {
        id: f.id.clone(),
        size: f.size,
        sha256: f.sha256.clone(),
    }
}

/// A step's runs by index: the proof of each that ran, and the cache key of each that was
/// answered from the cache.
fn step_runs(step: &StepState, proofs: &BTreeMap<String, StepRunProof>) -> Vec<RunEntry> {
    let mut runs: Vec<RunEntry> = step
        .runs
        .iter()
        .filter_map(|id| proofs.get(id).cloned().map(|p| RunEntry::Ran(Box::new(p))))
        .collect();
    runs.extend(step.cached_keys.iter().map(|(index, key)| {
        RunEntry::Cached(CachedRun {
            index: *index,
            cached: true,
            cache_key: key.clone(),
        })
    }));
    runs.sort_by_key(|r| match r {
        RunEntry::Ran(p) => p.index,
        RunEntry::Cached(c) => c.index,
    });
    runs
}

/// The receipt of an ended pipeline run of `app`, made by the cluster `cluster` (hex identity).
#[must_use]
pub fn build_receipt(
    cluster: &str,
    app: &str,
    run: &PipelineRun,
    proofs: &BTreeMap<String, StepRunProof>,
) -> PipelineReceipt {
    let mut modules = run.modules.clone();
    let _prev = modules.insert(SELF_MODULE.to_owned(), run.compute_bundle.clone());
    PipelineReceipt {
        version: RECEIPT_VERSION,
        cluster: cluster.to_owned(),
        app: app.to_owned(),
        run: run.id.clone(),
        pipeline: run.pipeline.clone(),
        release: run.version.clone(),
        pipeline_hash: run.hash.clone(),
        modules,
        inputs: run
            .inputs
            .iter()
            .map(|(name, ids)| {
                let files = ids
                    .iter()
                    .filter_map(|id| run.input_files.get(id))
                    .map(file_ref)
                    .collect();
                (name.clone(), files)
            })
            .collect(),
        params: run.params.clone(),
        state: name_of(&run.state),
        start_ts_ms: run.start_ts_ms,
        end_ts_ms: run.end_ts_ms.unwrap_or_default(),
        steps: run
            .steps
            .iter()
            .map(|(name, step)| {
                (
                    name.clone(),
                    StepReceipt {
                        status: name_of(&step.status),
                        reason: step.reason.clone(),
                        runs: step_runs(step, proofs),
                    },
                )
            })
            .collect(),
        cost_so_far: run.cost.map(|_| run.cost_so_far.to_string()),
    }
}

/// The signed receipt of a pipeline run that has ended. Only those who may handle the app's files
/// read it: it names every file's size and hash.
pub(super) async fn receipt(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, run)): Path<(String, String)>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = super::pipelines::component_app(&node, &app_id).await?;
    let p = gate(&node, &headers, &app, "*", false).await?;
    if !node.sys(|s| may_handle_files(s, &app, p.caller.account())) {
        return Err(ApiError::forbidden(
            "only those who handle the app's files read a receipt",
        ));
    }
    let answer = node
        .view(&crate::node::ViewRequest {
            app_id: app_id.clone(),
            method: PIPELINE_VIEW.to_owned(),
            args: json!({ "run": run, "files": true, "receipt": true }),
            caller: p.caller,
            strong: true,
            min_index: 0,
        })
        .await?;
    if answer.result.is_null() {
        return Err(ApiError::not_found("no such pipeline run"));
    }
    let source: ReceiptSource =
        serde_json::from_value(answer.result).map_err(|e| ApiError::internal(&e.to_string()))?;
    if source.run.state == PipelineState::Running {
        return Err(ApiError::new(
            409,
            RUN_NOT_ENDED,
            "the pipeline run is still running; a receipt covers a finished run",
        ));
    }
    let receipt = build_receipt(
        &hex::encode(node.cluster_identity()),
        &app_id,
        &source.run,
        &source.proofs,
    );
    let receipt: Value =
        serde_json::to_value(&receipt).map_err(|e| ApiError::internal(&e.to_string()))?;
    Ok(Json(node.sign_pipeline_receipt(receipt)))
}
