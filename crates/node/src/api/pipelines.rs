//! `POST /apps/{app}/pipelines/{name}/runs`, `GET /apps/{app}/pipeline-runs[/{run}]` and
//! `POST /apps/{app}/pipeline-runs/{run}/cancel`: starting, reading and cancelling a pipeline
//! run of the release the app runs. Those who may handle the app's files start, cancel and
//! retry; who may reach it reads. A start is judged against the pinned pipeline first.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use calimero_pipeline::{
    worst_case_cost, Caps, Env, InputKind, Limits, Param, ParamType, Pipeline,
};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{gate, may_handle_files, read_only_in, typed_id, ApiResult};
use crate::app::Status;
use crate::component::{CostCap, PipelineStartEntry, INPUT_NOT_HELD, TOO_MANY_PIPELINE_RUNS};
use crate::error::ApiError;
use crate::node::{Node, PIPELINE_VIEW};
use crate::packages::pipelines::SELF_MODULE;
use crate::types::{AppRecord, Caller, Executor};

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
const BAD_LIMITS: &str = "BAD_LIMITS";
const LIMIT_EXCEEDS_CAP: &str = "LIMIT_EXCEEDS_CAP";
const STILL_RUNNING: &str = "STILL_RUNNING";

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

/// The compute methods of each module a pipeline names, by the name as written.
type Methods = BTreeMap<String, BTreeMap<String, Caps>>;

/// `self` (the run's own compute component) and every pinned module with their methods.
async fn module_methods(
    node: &Node,
    compute_bundle: &str,
    modules: &BTreeMap<String, String>,
) -> Result<Methods, ApiError> {
    let mut out = Methods::new();
    let own = (SELF_MODULE, compute_bundle);
    for (name, hash) in
        std::iter::once(own).chain(modules.iter().map(|(n, h)| (n.as_str(), h.as_str())))
    {
        let methods = node.caps_of_component(hash).await?;
        let _prev = out.insert(name.to_owned(), (*methods).clone());
    }
    Ok(out)
}

/// The cost cap of a start: the operator's price table read fresh, the pipeline's worst case
/// under it, and the cap. A cap with no table is refused.
async fn cost_cap(
    node: &Node,
    pipeline: &Pipeline,
    modules: &Methods,
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
    let env = Env {
        default_module: Some(SELF_MODULE.to_owned()),
        modules: modules.clone(),
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

/// A run reads the app's sealed files and spends its payer's compute: only those who may handle
/// the app's files start, cancel or retry one, and never from a read-only team row.
async fn may_run(
    node: &Node,
    app: &crate::types::AppRecord,
    account: &str,
    sys_index: Option<u64>,
) -> Result<(), ApiError> {
    if !node.sys(|s| may_handle_files(s, app, Some(account))) {
        return Err(ApiError::forbidden(
            "only the app's owner and its organization's members run its pipelines",
        ));
    }
    if read_only_in(node, app, Some(account), sys_index).await {
        return Err(ApiError::forbidden("read-only in this team"));
    }
    Ok(())
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
    may_run(&node, &app, account, p.sys_index).await?;
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
    let modules = registered
        .modules
        .iter()
        .map(|(name, pin)| (name.clone(), pin.compute.clone()))
        .collect();
    let pipeline = node.pipeline_by_hash(&registered.hash).await?;
    let inputs = check_inputs(&pipeline, &body.inputs)?;
    let params = check_params(&pipeline, &body.params)?;
    let launch = Launch {
        name,
        version: release.at.version.clone(),
        hash: registered.hash.clone(),
        compute_bundle,
        modules,
        pipeline,
        inputs,
        params,
        limits: BTreeMap::new(),
        retry_of: None,
        max_cost: body.max_cost,
    };
    start_launch(&node, &app, p.caller, p.sys_index, launch).await
}

/// Everything a new pipeline run is made of: what a start takes from the request and the pinned
/// release, or a retry from the run it repeats.
struct Launch {
    name: String,
    version: String,
    hash: String,
    compute_bundle: String,
    modules: BTreeMap<String, String>,
    pipeline: Arc<Pipeline>,
    inputs: BTreeMap<String, Vec<String>>,
    params: BTreeMap<String, Value>,
    limits: BTreeMap<String, Limits>,
    retry_of: Option<String>,
    max_cost: Option<String>,
}

/// Checks the inputs are held, prices the cost cap and logs the start on the app's leader.
async fn start_launch(
    node: &Node,
    app: &AppRecord,
    caller: Caller,
    sys_index: Option<u64>,
    launch: Launch,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let Launch {
        name,
        version,
        hash,
        compute_bundle,
        modules,
        pipeline,
        inputs,
        params,
        limits,
        retry_of,
        max_cost,
    } = launch;
    for id in inputs.values().flatten() {
        let raw = <[u8; 32]>::try_from(hex::decode(id).unwrap_or_default())
            .ok()
            .filter(|raw| hex::encode(raw) == *id)
            .ok_or_else(|| ApiError::bad_request(BAD_INPUTS, "an input is not a file id"))?;
        if node.file_held_strong(app, &raw).await != Some(true) {
            return Err(expired());
        }
    }
    let priced = with_limits(&pipeline, &limits);
    let methods = module_methods(node, &compute_bundle, &modules).await?;
    let cost = cost_cap(node, &priced, &methods, max_cost.as_deref()).await?;
    let run = typed_id::<12>();
    let entry = PipelineStartEntry {
        id: run.clone(),
        pipeline: name,
        version,
        hash,
        compute_bundle,
        modules,
        owner: app.owner.clone(),
        steps: pipeline.steps.keys().cloned().collect(),
        inputs,
        params,
        caller: caller.clone(),
        payer: crate::usage::payer(app),
        fail_fast: pipeline.fail_fast,
        cost,
        retry_of: retry_of.clone(),
        limits,
        ts_ms: 0,
    };
    let a = node
        .start_pipeline_run(app, entry, caller, sys_index)
        .await?;
    match (a.status, a.error.as_str()) {
        (Status::Ok, _) => {
            let mut answer = json!({ "run": run });
            if let Some(cap) = cost {
                answer["worstCase"] = json!(cap.worst_case.to_string());
            }
            if let Some(of) = retry_of {
                answer["retryOf"] = json!(of);
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

/// The pipeline with each step's limits overridden, for pricing what a retry may cost.
fn with_limits(pipeline: &Pipeline, limits: &BTreeMap<String, Limits>) -> Pipeline {
    let mut priced = pipeline.clone();
    for (name, over) in limits {
        if let Some(step) = priced.steps.get_mut(name) {
            step.limits = Some(step.limits.unwrap_or_default().overridden_by(*over));
        }
    }
    priced
}

/// The limits of a retry: only steps of the pipeline, each field above zero and within the
/// method's caps.
fn check_limits(
    pipeline: &Pipeline,
    modules: &Methods,
    limits: &BTreeMap<String, Limits>,
) -> Result<(), ApiError> {
    let bad = |message: String| ApiError::bad_request(BAD_LIMITS, &message);
    for (name, l) in limits {
        let step = pipeline
            .steps
            .get(name)
            .ok_or_else(|| bad(format!("`{name}` is not a step of the pipeline")))?;
        let module = step.module.as_deref().or(pipeline.module.as_deref());
        let cap = modules
            .get(module.unwrap_or(SELF_MODULE))
            .and_then(|methods| methods.get(&step.method))
            .ok_or_else(|| bad(format!("step `{name}` runs a method the component lacks")))?;
        let given = [
            ("fuel", l.fuel.map(u128::from), u128::from(cap.fuel)),
            (
                "memoryMib",
                l.memory_mib.map(u128::from),
                u128::from(cap.memory_mib),
            ),
            (
                "wallSecs",
                l.wall_secs.map(u128::from),
                u128::from(cap.wall_secs),
            ),
        ];
        for (field, value, most) in given {
            match value {
                Some(0) => return Err(bad(format!("`{name}.{field}` must be above zero"))),
                Some(v) if v > most => {
                    return Err(ApiError::bad_request(
                        LIMIT_EXCEEDS_CAP,
                        &format!("`{name}.{field}` is above the method's cap of {most}"),
                    ))
                }
                _ => {}
            }
        }
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RetryBody {
    #[serde(default)]
    limits: BTreeMap<String, Limits>,
    #[serde(default)]
    max_cost: Option<String>,
}

/// Starts a new run of the same pipeline, release, inputs and params as an ended run, with the
/// given limits raised; the step cache answers what already succeeded.
pub(super) async fn retry(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, run)): Path<(String, String)>,
    Json(body): Json<RetryBody>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let app_id = node.canonical_app_id(&app_id);
    let app = component_app(&node, &app_id).await?;
    let p = gate(&node, &headers, &app, "*", true).await?;
    let Some(account) = p.caller.account() else {
        return Err(ApiError::unauthenticated("sign in to retry a pipeline run"));
    };
    may_run(&node, &app, account, p.sys_index).await?;
    node.require_upgraded_nodes().await?;
    let answer = node
        .view(&crate::node::ViewRequest {
            app_id,
            method: PIPELINE_VIEW.to_owned(),
            args: json!({ "run": run, "files": false }),
            caller: p.caller.clone(),
            strong: true,
            min_index: 0,
        })
        .await?;
    let original = answer.result;
    if original.is_null() {
        return Err(ApiError::not_found("no such pipeline run"));
    }
    if original["state"] == "running" {
        return Err(ApiError::new(
            409,
            STILL_RUNNING,
            "the pipeline run is still running; cancel it or wait",
        ));
    }
    let field = |name: &str| -> Result<String, ApiError> {
        original[name]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| ApiError::internal("the pipeline run's record is not readable"))
    };
    let unreadable = |_| ApiError::internal("the pipeline run's record is not readable");
    let hash = field("hash")?;
    let compute_bundle = field("computeBundle")?;
    let pipeline = node.pipeline_by_hash(&hash).await?;
    let modules: BTreeMap<String, String> = match &original["modules"] {
        Value::Null => BTreeMap::new(),
        v => serde_json::from_value(v.clone()).map_err(unreadable)?,
    };
    let methods = module_methods(&node, &compute_bundle, &modules).await?;
    check_limits(&pipeline, &methods, &body.limits)?;
    let launch = Launch {
        name: field("pipeline")?,
        version: field("version")?,
        hash,
        compute_bundle,
        modules,
        pipeline,
        inputs: serde_json::from_value(original["inputs"].clone()).map_err(unreadable)?,
        params: serde_json::from_value(original["params"].clone()).map_err(unreadable)?,
        limits: body.limits,
        retry_of: Some(run),
        max_cost: body.max_cost,
    };
    start_launch(&node, &app, p.caller, p.sys_index, launch).await
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
    may_run(&node, &app, account, p.sys_index).await?;
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
    fn limits_are_for_steps_above_zero_and_within_the_methods_caps() {
        let p = pipeline();
        let methods = Methods::from([(
            SELF_MODULE.to_owned(),
            BTreeMap::from([(
                "m".to_owned(),
                Caps {
                    fuel: 100,
                    memory_mib: 64,
                    wall_secs: 30,
                },
            )]),
        )]);
        let of = |step: &str, l: Limits| BTreeMap::from([(step.to_owned(), l)]);
        let fuel = |n| Limits {
            fuel: Some(n),
            ..Limits::default()
        };
        assert!(
            check_limits(&p, &methods, &of("one", fuel(100))).is_ok(),
            "at the cap"
        );
        assert!(check_limits(&p, &methods, &BTreeMap::new()).is_ok());
        for (name, limits, code) in [
            ("over the cap", of("one", fuel(101)), LIMIT_EXCEEDS_CAP),
            ("zero", of("one", fuel(0)), BAD_LIMITS),
            ("not a step", of("two", fuel(1)), BAD_LIMITS),
            (
                "memory over",
                of(
                    "one",
                    Limits {
                        memory_mib: Some(65),
                        ..Limits::default()
                    },
                ),
                LIMIT_EXCEEDS_CAP,
            ),
            (
                "wall over",
                of(
                    "one",
                    Limits {
                        wall_secs: Some(31),
                        ..Limits::default()
                    },
                ),
                LIMIT_EXCEEDS_CAP,
            ),
        ] {
            let err = check_limits(&p, &methods, &limits).unwrap_err();
            assert_eq!((err.status, err.code.as_str()), (400, code), "{name}");
        }
    }

    #[test]
    fn a_retrys_worst_case_is_priced_with_its_raised_limits() {
        let mut p = pipeline();
        let step = p.steps.get_mut("one").unwrap();
        step.limits = Some(Limits {
            fuel: Some(10),
            wall_secs: Some(5),
            ..Limits::default()
        });
        let raised = BTreeMap::from([(
            "one".to_owned(),
            Limits {
                fuel: Some(50),
                ..Limits::default()
            },
        )]);
        let priced = with_limits(&p, &raised);
        let l = priced.steps["one"].limits.unwrap();
        assert_eq!(
            (l.fuel, l.wall_secs),
            (Some(50), Some(5)),
            "only the named field moves"
        );
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
