//! `meroctl pipeline`: start, read and cancel the pipeline runs of an app instance. A pipeline
//! is registered by the release the instance runs (`meroctl app pipelines`); its inputs are the
//! instance's sealed files (`meroctl file upload`).

use std::collections::BTreeMap;
use std::time::Duration;

use clap::Subcommand;
use eyre::{bail, eyre};
use serde_json::{json, Value};

use crate::env::Env;

const WAIT_POLL: Duration = Duration::from_secs(1);

#[derive(Debug, Subcommand)]
pub enum PipelineCmd {
    /// Start a run of the pipeline `name`; prints `{ run }`, or with `--wait` the ended run.
    Run {
        /// The context (alias or id).
        app: String,
        name: String,
        /// Bind an input to file ids: `NAME=ID[,ID...]`; once per input.
        #[arg(long = "input", value_name = "NAME=ID[,ID...]")]
        inputs: Vec<String>,
        /// Give a param: `NAME=VALUE` (a number or boolean as such, else text); once per param.
        #[arg(long = "param", value_name = "NAME=VALUE")]
        params: Vec<String>,
        /// Start no step run that could take the run past this many micro-units.
        #[arg(long, value_name = "MICRO_UNITS")]
        max_cost: Option<String>,
        /// Wait until the run is no longer running and print it.
        #[arg(long)]
        wait: bool,
    },
    /// Start a new run of an ended run's pipeline, release, inputs and params with some limits
    /// raised; steps that already succeeded are answered from the step cache.
    Retry {
        app: String,
        run: String,
        /// Raise a step's limit: `STEP.fuel=N`, `STEP.memoryMib=N` or `STEP.wallSecs=N`; repeatable.
        #[arg(long = "limit", value_name = "STEP.FIELD=N")]
        limits: Vec<String>,
        /// Start no step run that could take the run past this many micro-units.
        #[arg(long, value_name = "MICRO_UNITS")]
        max_cost: Option<String>,
        /// Wait until the new run is no longer running and print it.
        #[arg(long)]
        wait: bool,
    },
    /// One run: its state and each step's status, runs, outputs, reason, cost and what the step cache answered.
    Get { app: String, run: String },
    /// Cancel a run: its pending steps end cancelled and its running steps' runs stop.
    Cancel { app: String, run: String },
    /// The context's recent runs, newest first.
    Runs { app: String },
}

fn split(arg: &str) -> eyre::Result<(&str, &str)> {
    arg.split_once('=')
        .filter(|(name, _)| !name.is_empty())
        .ok_or_else(|| eyre!("`{arg}` is not NAME=VALUE"))
}

/// `--input NAME=ID,ID` as the files bound to each input.
pub fn parse_inputs(args: &[String]) -> eyre::Result<Value> {
    let mut inputs: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for arg in args {
        let (name, ids) = split(arg)?;
        inputs
            .entry(name)
            .or_default()
            .extend(ids.split(',').filter(|id| !id.is_empty()));
    }
    Ok(json!(inputs))
}

/// `--param NAME=VALUE`, a value that reads as JSON (a number, a boolean) as that, else as text.
pub fn parse_params(args: &[String]) -> eyre::Result<Value> {
    let mut params = serde_json::Map::new();
    for arg in args {
        let (name, value) = split(arg)?;
        let value = serde_json::from_str(value)
            .ok()
            .filter(|v: &Value| v.is_number() || v.is_boolean())
            .unwrap_or_else(|| json!(value));
        if params.insert(name.to_owned(), value).is_some() {
            bail!("param `{name}` is given twice");
        }
    }
    Ok(Value::Object(params))
}

/// `--limit STEP.FIELD=N` as `{ STEP: { FIELD: N } }`; the node judges the field and the bounds.
fn parse_limits(args: &[String]) -> eyre::Result<Value> {
    let mut limits: BTreeMap<&str, serde_json::Map<String, Value>> = BTreeMap::new();
    for arg in args {
        let (path, value) = split(arg)?;
        let (step, field) = path
            .rsplit_once('.')
            .filter(|(step, field)| !step.is_empty() && !field.is_empty())
            .ok_or_else(|| eyre!("`{path}` is not STEP.FIELD"))?;
        let n: u64 = value
            .parse()
            .map_err(|_| eyre!("`{value}` is not a whole number"))?;
        if limits
            .entry(step)
            .or_default()
            .insert(field.to_owned(), json!(n))
            .is_some()
        {
            bail!("limit `{path}` is given twice");
        }
    }
    Ok(json!(limits))
}

/// Polls a started run until it is no longer running and returns it.
async fn wait_for(env: &Env, app: &str, started: &Value) -> eyre::Result<Value> {
    let id = started["run"]
        .as_str()
        .ok_or_else(|| eyre!("the node named no run"))?;
    loop {
        let run = env.client.pipeline_run(app, id).await?;
        if run["state"] != "running" {
            return Ok(run);
        }
        tokio::time::sleep(WAIT_POLL).await;
    }
}

/// The body of a start: the inputs, the params and the cost cap when one is given.
fn start_body(inputs: &[String], params: &[String], max_cost: Option<&str>) -> eyre::Result<Value> {
    let mut body = json!({ "inputs": parse_inputs(inputs)?, "params": parse_params(params)? });
    if let Some(max) = max_cost {
        body["maxCost"] = json!(max);
    }
    Ok(body)
}

pub async fn run(cmd: PipelineCmd, env: &Env) -> eyre::Result<Value> {
    Ok(match cmd {
        PipelineCmd::Run {
            app,
            name,
            inputs,
            params,
            max_cost,
            wait,
        } => {
            let app = env.app(&app).await?;
            let body = start_body(&inputs, &params, max_cost.as_deref())?;
            let started = env
                .client
                .start_pipeline_run_with(&app, &name, body)
                .await?;
            if !wait {
                return Ok(started);
            }
            wait_for(env, &app, &started).await?
        }
        PipelineCmd::Retry {
            app,
            run,
            limits,
            max_cost,
            wait,
        } => {
            let app = env.app(&app).await?;
            let mut body = json!({ "limits": parse_limits(&limits)? });
            if let Some(max) = max_cost {
                body["maxCost"] = json!(max);
            }
            let started = env.client.retry_pipeline_run(&app, &run, body).await?;
            if !wait {
                return Ok(started);
            }
            wait_for(env, &app, &started).await?
        }
        PipelineCmd::Get { app, run } => {
            env.client.pipeline_run(&env.app(&app).await?, &run).await?
        }
        PipelineCmd::Cancel { app, run } => {
            env.client
                .cancel_pipeline_run(&env.app(&app).await?, &run)
                .await?
        }
        PipelineCmd::Runs { app } => env.client.pipeline_runs(&env.app(&app).await?).await?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| (*a).to_owned()).collect()
    }

    #[test]
    fn inputs_bind_names_to_file_ids() {
        let v = parse_inputs(&strings(&["docs=a,b", "docs=c", "model=m"])).unwrap();
        assert_eq!(v, json!({ "docs": ["a", "b", "c"], "model": ["m"] }));
        assert!(parse_inputs(&strings(&["docs"])).is_err());
        assert!(parse_inputs(&strings(&["=a"])).is_err());
    }

    #[test]
    fn limits_name_a_step_and_a_field_and_a_whole_number() {
        let v = parse_limits(&strings(&["a.fuel=9", "a.wallSecs=3", "b.memoryMib=64"])).unwrap();
        assert_eq!(
            v,
            json!({ "a": { "fuel": 9, "wallSecs": 3 }, "b": { "memoryMib": 64 } })
        );
        assert_eq!(parse_limits(&[]).unwrap(), json!({}));
        for bad in [
            &["fuel=9"][..],
            &["a.fuel=x"],
            &["a.fuel"],
            &[".fuel=1"],
            &["a.fuel=1", "a.fuel=2"],
        ] {
            assert!(parse_limits(&strings(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_cost_cap_is_sent_as_the_decimal_text_given() {
        let body = start_body(&strings(&["docs=a"]), &[], Some("5000")).unwrap();
        assert_eq!(body["maxCost"], json!("5000"));
        assert_eq!(body["inputs"], json!({ "docs": ["a"] }));
        assert!(start_body(&[], &[], None).unwrap().get("maxCost").is_none());
    }

    #[test]
    fn params_read_numbers_and_booleans_and_keep_the_rest_as_text() {
        let v = parse_params(&strings(&[
            "t=0.5",
            "on=true",
            "mode=fast",
            "n=\"7\"",
            "s=1e",
        ]))
        .unwrap();
        assert_eq!(
            v,
            json!({ "t": 0.5, "on": true, "mode": "fast", "n": "\"7\"", "s": "1e" })
        );
        assert!(parse_params(&strings(&["t=1", "t=2"])).is_err());
        assert!(parse_params(&strings(&["t"])).is_err());
    }
}
