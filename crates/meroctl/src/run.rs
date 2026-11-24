//! `meroctl run`: a compute run's module log.

use clap::Subcommand;
use eyre::{bail, eyre};
use serde_json::Value;

use crate::env::Env;

#[derive(Debug, Subcommand)]
pub enum RunCmd {
    /// Print a compute run's module log (stdout, stderr and `log` lines) to stdout, where an
    /// earlier release kept one: a current cluster keeps none.
    Logs {
        /// The app instance (alias or id).
        app: String,
        run: String,
        /// The attempt (default: the latest that kept a log).
        #[arg(long)]
        attempt: Option<u32>,
    },
}

/// The id of the log of `attempt` among the `logs` a node listed, else of the latest attempt.
fn pick_log(listed: &Value, attempt: Option<u32>) -> eyre::Result<String> {
    let logs = listed["logs"].as_array().map_or(&[][..], Vec::as_slice);
    let log = match attempt {
        Some(n) => logs.iter().find(|l| l["attempt"] == n),
        None => logs.iter().max_by_key(|l| l["attempt"].as_u64()),
    };
    match (log, attempt) {
        (Some(log), _) => log["id"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| eyre!("the node listed a log without an id")),
        (None, Some(n)) => bail!("attempt {n} kept no log: {NO_RUN_LOGS}"),
        (None, None) => bail!("the run kept no log: {NO_RUN_LOGS}"),
    }
}

/// Why a run lists no log on a current cluster.
const NO_RUN_LOGS: &str = "a cluster keeps no compute run's log, as an app's log lines may hold \
     user data; run the method on your own machine with `meroctl local compute run` to see them";

/// Writes the log of compute run `run` of `app` to stdout: attempt `attempt`, else the latest.
pub async fn print_log(env: &Env, app: &str, run: &str, attempt: Option<u32>) -> eyre::Result<()> {
    let listed = serde_json::to_value(env.client.apps().run_logs(app, run).await?)?;
    let id = pick_log(&listed, attempt)?;
    let bytes = env.client.export_file(app, &id).await?;
    std::io::Write::write_all(&mut std::io::stdout(), &bytes)?;
    Ok(())
}

pub async fn run(cmd: RunCmd, env: &Env) -> eyre::Result<Value> {
    match cmd {
        RunCmd::Logs { app, run, attempt } => {
            print_log(env, &env.app(&app).await?, &run, attempt).await?;
        }
    }
    Ok(Value::Null)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_latest_attempt_is_read_unless_one_is_named() {
        let listed = json!({ "logs": [
            { "attempt": 1, "id": "a" },
            { "attempt": 3, "id": "c" },
            { "attempt": 2, "id": "b" },
        ] });
        assert_eq!(pick_log(&listed, None).unwrap(), "c");
        assert_eq!(pick_log(&listed, Some(2)).unwrap(), "b");
        assert!(pick_log(&listed, Some(4)).is_err());
        assert!(pick_log(&json!({}), None).is_err());
    }

    /// A current cluster keeps no run log and lists none: the command says so and why.
    #[test]
    fn a_run_without_a_log_says_the_cluster_keeps_none() {
        let e = pick_log(&json!({ "logs": [] }), None).unwrap_err();
        let said = e.to_string();
        assert!(said.starts_with("the run kept no log: "), "{said}");
        assert!(said.contains("keeps no compute run's log"), "{said}");
        assert!(said.contains("meroctl local compute run"), "{said}");
    }
}
