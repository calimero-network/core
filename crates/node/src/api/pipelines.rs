//! `POST /apps/{app}/pipelines/{name}/runs`, `GET /apps/{app}/pipeline-runs[/{run}]` and
//! `POST /apps/{app}/pipeline-runs/{run}/cancel`: starting, reading and cancelling a pipeline
//! run of the release the app runs. Who may call the app starts and cancels; who may reach it
//! reads. A start is judged against the pinned pipeline before anything is logged.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use calimero_pipeline::{worst_case_cost, Env, InputKind, Param, ParamType, Pipeline};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{gate, may_handle_files, read_only_in, typed_id, ApiResult};
use crate::app::Status;
use crate::component::{CostCap, PipelineStartEntry, INPUT_NOT_HELD, TOO_MANY_PIPELINE_RUNS};
use crate::error::ApiError;
use crate::node::{Node, PIPELINE_VIEW};
use crate::types::Executor;

const MAX_INPUT_FILES: usize = 1024; // file ids across all inputs of one run, which the log entry carries
const MAX_PARAM_STRING_BYTES: usize = 4096; // a string param's value

/// The refusal for a pipeline the app's release does not register.
pub const NO_SUCH_PIPELINE: &str = "NO_SUCH_PIPELINE";
const BAD_INPUTS: &str = "BAD_INPUTS";
const OUTPUT_EXPIRED: &str = "OUTPUT_EXPIRED";
const BAD_PARAMS: &str = "BAD_PARAMS";
const ONE_SERVICE: &str = "PIPELINES_NEED_ONE_SERVICE";
const NO_PRICE_TABLE: &str = "NO_PRICE_TABLE";
const BAD_MAX_COST: &str = "BAD_MAX_COST";
/// The module name pipelines of a package see its own compute methods under.
const SELF_MODULE: &str = "self";

/// An input that is a file id the app does not hold: a file is forgotten once nothing refers to
/// it, so this cannot tell one that was never there from one that expired.
fn expired() -> ApiError {
    ApiError::new(
        410,
        OUTPUT_EXPIRED,
        "an input is not a file this app holds; it may have expired",
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct StartBody {
    #[serde(default)]
    inputs: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    params: BTreeMap<String, Value>,
    /// Micro-units, a decimal string: the run starts no step run that could take it past this.
    #[serde(default)]
    max_cost: Option<String>,
}

/// The cost cap of a start: the operator's price table read fresh, the pipeline's worst case
/// under it, and the cap. A cap with no table is refused.
async fn cost_cap(
    node: &Node,
    pipeline: &Pipeline,
    compute_bundle: &str,
    max_cost: Option<&str>,
) -> Result<Option<CostCap>, ApiError> {
    let Some(max) = max_cost else { return Ok(None) };
    let max_cost = max.parse::<u64>().map_err(|_| {
        ApiError::bad_request(BAD_MAX_COST, "maxCost is a whole number of micro-units")
    })?;
    let _caught_up = node.sys_fresh().await;
    let Some(table) = node.sys(|s| s.price_table) else {
        return Err(ApiError::new(
            409,
            NO_PRICE_TABLE,
            "the operator has not set a price table, so a cost cap cannot be priced",
        ));
    };
    let methods = node.caps_of_component(compute_bundle).await?;
    let env = Env {
        default_module: Some(SELF_MODULE.to_owned()),
        modules: BTreeMap::from([(SELF_MODULE.to_owned(), (*methods).clone())]),
        pools: BTreeMap::new(),
    };
    let worst = worst_case_cost(pipeline, &env, &table);
    Ok(Some(CostCap {
        max_cost,
        price_version: table.version,
        table,
        worst_case: u64::try_from(worst).unwrap_or(u64::MAX),
    }))
}

/// The bound inputs of a start, each within its declaration; an input left unbound must have a
/// default. Whether a file is held is the leader's to judge, not this function's.
pub fn check_inputs(
    pipeline: &Pipeline,
    given: &BTreeMap<String, Vec<String>>,
) -> Result<BTreeMap<String, Vec<String>>, ApiError> {
    let bad = |message: String| ApiError::bad_request(BAD_INPUTS, &message);
    if let Some(name) = given.keys().find(|n| !pipeline.inputs.contains_key(*n)) {
        return Err(bad(format!("`{name}` is not an input of the pipeline")));
    }
    let mut bound = BTreeMap::new();
    for (name, input) in &pipeline.inputs {
        let files = given.get(name).cloned().unwrap_or_default();
        if files.is_empty() && input.default.is_none() {
            return Err(bad(format!("input `{name}` needs at least one file")));
        }
        let most = match input.kind {
            InputKind::Files => input.max_items.map(|n| n as usize),
            InputKind::Artifact | InputKind::Table => Some(1),
        };
        if most.is_some_and(|m| files.len() > m) {
            return Err(bad(format!(
                "input `{name}` takes at most {} file(s)",
                most.unwrap_or(0)
            )));
        }
        let _prev = bound.insert(name.clone(), files);
    }
    if bound.values().map(Vec::len).sum::<usize>() > MAX_INPUT_FILES {
        return Err(bad(format!("a run takes at most {MAX_INPUT_FILES} files")));
    }
    Ok(bound)
}

fn param_fits(name: &str, param: &Param, value: &Value) -> Result<(), String> {
    let wrong = || format!("param `{name}` is not within its declaration");
    match (param.ty, value) {
        (ParamType::Number, Value::Number(n)) => {
            let x = n.as_f64().ok_or_else(wrong)?;
            if param.min.is_some_and(|m| x < m) || param.max.is_some_and(|m| x > m) {
                return Err(wrong());
            }
        }
        (ParamType::String, Value::String(s)) if s.len() <= MAX_PARAM_STRING_BYTES => {}
        (ParamType::Boolean, Value::Bool(_)) => {}
        _ => return Err(format!("param `{name}` has the wrong type")),
    }
    let same = |a: &Value, b: &Value| match (a.as_f64(), b.as_f64()) {
        (Some(a), Some(b)) => a == b,
        _ => a == b,
    };
    match &param.values {
        Some(allowed) if !allowed.iter().any(|a| same(a, value)) => Err(wrong()),
        _ => Ok(()),
    }
}

/// The params of a start: each declared one given and within its declaration, or its default.
pub fn check_params(
    pipeline: &Pipeline,
    given: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>, ApiError> {
    let bad = |message: String| ApiError::bad_request(BAD_PARAMS, &message);
    if let Some(name) = given.keys().find(|n| !pipeline.params.contains_key(*n)) {
        return Err(bad(format!("`{name}` is not a param of the pipeline")));
    }
    let mut out = BTreeMap::new();
    for (name, param) in &pipeline.params {
        let value = match given.get(name).or(param.default.as_ref()) {
            Some(v) => v.clone(),
            None => return Err(bad(format!("param `{name}` needs a value"))),
        };
        param_fits(name, param, &value).map_err(bad)?;
        let _prev = out.insert(name.clone(), value);
    }
    Ok(out)
}

async fn component_app(node: &Node, app_id: &str) -> Result<crate::types::AppRecord, ApiError> {
    let app = node.app(app_id).await?;
    if app.executor != Executor::Component {
        return Err(ApiError::not_found("no such pipeline run"));
    }
    Ok(app)
}

/// Starts a run of the named pipeline of the release the app runs.
pub(super) async fn start(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, name)): Path<(String, String)>,
    Json(body): Json<StartBody>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let app_id = node.canonical_app_id(&app_id);
    let app = component_app(&node, &app_id).await?;
    let p = gate(&node, &headers, &app, "*", true).await?;
    let Some(account) = p.caller.account() else {
        return Err(ApiError::unauthenticated("sign in to start a pipeline run"));
    };
    if read_only_in(&node, &app, Some(account), p.sys_index).await {
        return Err(ApiError::forbidden("read-only in this team"));
    }
    node.require_upgraded_nodes().await?;
    let no_pipeline = || {
        ApiError::new(
            404,
            NO_SUCH_PIPELINE,
            "the app's release has no such pipeline",
        )
    };
    let release = node.release_of_app(&app).ok_or_else(no_pipeline)?;
    let registered = release
        .version
        .pipelines
        .get(&name)
        .ok_or_else(no_pipeline)?;
    let compute_bundle = match (
        release.version.compute.len(),
        release.version.compute.get(""),
    ) {
        (1, Some(hash)) => hash.clone(),
        _ => {
            return Err(ApiError::new(
                422,
                ONE_SERVICE,
                "pipelines run on a package with one service",
            ))
        }
    };
    let pipeline = node.pipeline_by_hash(&registered.hash).await?;
    let inputs = check_inputs(&pipeline, &body.inputs)?;
    let params = check_params(&pipeline, &body.params)?;
    for id in inputs.values().flatten() {
        let raw = <[u8; 32]>::try_from(hex::decode(id).unwrap_or_default())
            .ok()
            .filter(|raw| hex::encode(raw) == *id)
            .ok_or_else(|| ApiError::bad_request(BAD_INPUTS, "an input is not a file id"))?;
        if node.file_held_strong(&app, &raw).await != Some(true) {
            return Err(expired());
        }
    }
    let cost = cost_cap(&node, &pipeline, &compute_bundle, body.max_cost.as_deref()).await?;
    let run = typed_id::<12>();
    let entry = PipelineStartEntry {
        id: run.clone(),
        pipeline: name,
        version: release.at.version.clone(),
        hash: registered.hash.clone(),
        compute_bundle,
        owner: app.owner.clone(),
        steps: pipeline.steps.keys().cloned().collect(),
        inputs,
        params,
        caller: p.caller.clone(),
        payer: crate::usage::payer(&app),
        fail_fast: pipeline.fail_fast,
        cost,
        ts_ms: 0,
    };
    let a = node
        .start_pipeline_run(&app, entry, p.caller, p.sys_index)
        .await?;
    match (a.status, a.error.as_str()) {
        (Status::Ok, _) => {
            let mut answer = json!({ "run": run });
            if let Some(cap) = cost {
                answer["worstCase"] = json!(cap.worst_case.to_string());
            }
            Ok((StatusCode::ACCEPTED, Json(answer)))
        }
        (_, Some(TOO_MANY_PIPELINE_RUNS)) => Err(ApiError::new(
            429,
            TOO_MANY_PIPELINE_RUNS,
            "this app has too many pipeline runs going; wait for one to end",
        )),
        (_, Some(INPUT_NOT_HELD)) => Err(expired()),
        (status, why) => Err(ApiError::conflict(&format!(
            "the pipeline run was not started ({status:?}: {})",
            why.unwrap_or("no reason")
        ))),
    }
}

/// One pipeline run's record, as the leader holds it.
pub(super) async fn get(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, run)): Path<(String, String)>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = component_app(&node, &app_id).await?;
    let p = gate(&node, &headers, &app, "*", false).await?;
    let files = node.sys(|s| may_handle_files(s, &app, p.caller.account()));
    let answer = node
        .view(&crate::node::ViewRequest {
            app_id,
            method: PIPELINE_VIEW.to_owned(),
            args: json!({ "run": run, "files": files }),
            caller: p.caller,
            strong: true,
            min_index: 0,
        })
        .await?;
    if answer.result.is_null() {
        return Err(ApiError::not_found("no such pipeline run"));
    }
    Ok(Json(answer.result))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ListQuery {
    #[serde(default)]
    limit: Option<String>,
}

/// The app's recent pipeline runs, newest first.
pub(super) async fn list(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Query(q): Query<ListQuery>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = component_app(&node, &app_id).await?;
    let p = gate(&node, &headers, &app, "*", false).await?;
    let files = node.sys(|s| may_handle_files(s, &app, p.caller.account()));
    let limit = match q.limit {
        None => Value::Null,
        Some(l) => json!(l
            .parse::<u64>()
            .map_err(|_| ApiError::bad_request("BAD_LIMIT", "limit is a whole number"))?),
    };
    let answer = node
        .view(&crate::node::ViewRequest {
            app_id,
            method: PIPELINE_VIEW.to_owned(),
            args: json!({ "limit": limit, "files": files }),
            caller: p.caller,
            strong: true,
            min_index: 0,
        })
        .await?;
    Ok(Json(answer.result))
}

/// Cancels a pipeline run: its pending steps end cancelled and its running steps' runs stop.
pub(super) async fn cancel(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, run)): Path<(String, String)>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = component_app(&node, &app_id).await?;
    let p = gate(&node, &headers, &app, "*", true).await?;
    let Some(account) = p.caller.account() else {
        return Err(ApiError::unauthenticated(
            "sign in to cancel a pipeline run",
        ));
    };
    if read_only_in(&node, &app, Some(account), p.sys_index).await {
        return Err(ApiError::forbidden("read-only in this team"));
    }
    let a = node
        .cancel_pipeline_run(&app, &run, p.caller, p.sys_index)
        .await?;
    if a.status != Status::Ok {
        return Err(ApiError::not_found("no pipeline run is going by that id"));
    }
    Ok(Json(json!({ "run": run })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pipeline() -> Pipeline {
        let source = r#"
apiVersion: compute.calimero.network/v1
kind: Pipeline
name: demo
params:
  threshold: { type: number, default: 0.5, min: 0, max: 1 }
  mode: { type: string, enum: [fast, slow], default: fast }
  flag: { type: boolean }
inputs:
  docs: { kind: files, maxItems: 2 }
  model: { kind: artifact, default: m@1 }
steps:
  one: { method: m, inputs: [inputs.docs] }
"#;
        calimero_pipeline::compile(source).unwrap()
    }

    fn files(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("{i:064x}")).collect()
    }

    #[test]
    fn inputs_must_be_declared_bound_and_within_their_limits() {
        let p = pipeline();
        let given = |docs: usize| BTreeMap::from([("docs".to_owned(), files(docs))]);
        let ok = check_inputs(&p, &given(2)).unwrap();
        assert_eq!(ok["docs"].len(), 2);
        assert_eq!(
            ok["model"].len(),
            0,
            "an input with a default may stay unbound"
        );
        for (name, bad) in [
            ("unbound", BTreeMap::new()),
            ("too many", given(3)),
            ("unknown", BTreeMap::from([("nope".to_owned(), files(1))])),
            (
                "an artifact takes one",
                BTreeMap::from([
                    ("docs".to_owned(), files(1)),
                    ("model".to_owned(), files(2)),
                ]),
            ),
        ] {
            assert_eq!(
                check_inputs(&p, &bad).unwrap_err().code,
                BAD_INPUTS,
                "{name}"
            );
        }
    }

    #[test]
    fn params_are_typed_bounded_and_defaulted() {
        let p = pipeline();
        let run = |given: &[(&str, Value)]| {
            let mut map: BTreeMap<String, Value> = given
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect();
            let _ = map.entry("flag".to_owned()).or_insert(json!(true));
            check_params(&p, &map)
        };
        let ok = run(&[]).unwrap();
        assert_eq!(ok["threshold"], json!(0.5));
        assert_eq!(ok["mode"], json!("fast"));
        assert_eq!(
            run(&[("threshold", json!(1))]).unwrap()["threshold"],
            json!(1)
        );
        for (name, bad) in [
            ("above max", run(&[("threshold", json!(1.5))])),
            ("below min", run(&[("threshold", json!(-1))])),
            ("wrong type", run(&[("threshold", json!("high"))])),
            ("not in enum", run(&[("mode", json!("medium"))])),
            ("unknown", run(&[("zzz", json!(1))])),
            (
                "no value and no default",
                check_params(&p, &BTreeMap::new()),
            ),
        ] {
            assert_eq!(bad.unwrap_err().code, BAD_PARAMS, "{name}");
        }
    }
}
