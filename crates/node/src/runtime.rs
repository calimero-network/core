//! Which nodes can run a release: its signed `minRuntimeVersion` against each
//! node's runtime, the merod release it was built as (`Config::release`).
//!
//! A bundle's requirement is checked when a node verifies its `.mpk`
//! (`packages::verify`), but only that node verifies it: the others take the
//! WASM. So placement, replica moves and upgrades judge the requirement here,
//! against each node's runtime as it answers it (`Node::runtimes`), and keep
//! an app off a node that falls short of the release it runs or moves to.

use std::collections::BTreeMap;

use calimero_log::NodeId;
use semver::Version;
use serde_json::{json, Value};

use crate::error::ApiError;

/// The refusal (`409`) of an upgrade, a placement or a replica move that
/// would have a node run a release needing a newer runtime than it has.
pub const RUNTIME_TOO_OLD: &str = "RUNTIME_TOO_OLD";

/// The most a peer's answer naming its runtime (`{ "runtime": "..." }`) may
/// hold, in bytes; a longer one is not read.
pub const MAX_RUNTIME_ANSWER_BYTES: usize = 256;

/// What a node's runtime is known to be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeRuntime {
    /// The release it names.
    Known(Version),
    /// Older than this one: the node answered, but does not know the
    /// question, so it runs a release from before runtimes were asked for,
    /// older than the asking node's own.
    Before(Version),
    /// Not known: no answer, or a release that is not a version (a
    /// development build). Judged as meeting every requirement, as before
    /// runtimes were asked for.
    Unknown,
}

impl NodeRuntime {
    /// A runtime as a node names it; [`Self::Unknown`] when it is no version.
    #[must_use]
    pub fn named(release: &str) -> Self {
        Version::parse(release).map_or(Self::Unknown, Self::Known)
    }

    /// Whether this runtime is known to be older than `min`.
    #[must_use]
    pub fn short_of(&self, min: &Version) -> bool {
        match self {
            Self::Known(v) => v < min,
            // Older than `v`, which is not newer than `min`.
            Self::Before(v) => v <= min,
            Self::Unknown => false,
        }
    }

    /// As a report or a refusal names it: the version, or `{ "before": v }`.
    #[must_use]
    pub fn to_json(&self) -> Value {
        match self {
            Self::Known(v) => json!(v.to_string()),
            Self::Before(v) => json!({ "before": v.to_string() }),
            Self::Unknown => Value::Null,
        }
    }
}

/// A manifest's `minRuntimeVersion`, when it is a version (a signed manifest's
/// always is: `packages::verify` refuses another).
#[must_use]
pub fn required(min: &str) -> Option<Version> {
    Version::parse(min).ok()
}

/// The nodes of `nodes` known to run a runtime older than `min`, each with
/// what it runs (`{ node, runtime }`), in the order given.
#[must_use]
pub fn short_of(
    nodes: &[NodeId],
    runtimes: &BTreeMap<NodeId, NodeRuntime>,
    min: &Version,
) -> Vec<Value> {
    nodes
        .iter()
        .filter_map(|id| {
            let rt = runtimes.get(id)?;
            rt.short_of(min)
                .then(|| json!({ "node": id, "runtime": rt.to_json() }))
        })
        .collect()
}

/// `409 RUNTIME_TOO_OLD`: `what` needs runtime `min`, which `nodes` (from
/// [`short_of`]) fall short of.
#[must_use]
pub fn refusal(what: &str, min: &Version, nodes: Vec<Value>) -> ApiError {
    let named: Vec<String> = nodes
        .iter()
        .map(|n| match &n["runtime"] {
            Value::String(v) => format!("node {} runs {v}", n["node"]),
            r => format!(
                "node {} runs a release older than {}",
                n["node"],
                r["before"].as_str().unwrap_or("this one")
            ),
        })
        .collect();
    ApiError::new(
        409,
        RUNTIME_TOO_OLD,
        &format!(
            "{what} needs runtime {min} or newer, and {}",
            named.join(", ")
        ),
    )
    .with_details(json!({ "minRuntimeVersion": min.to_string(), "nodes": nodes }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn a_runtime_falls_short_only_when_known_to_be_older() {
        let min = v("1.0.0-rc.16");
        assert!(NodeRuntime::named("1.0.0-rc.15").short_of(&min));
        assert!(!NodeRuntime::named("1.0.0-rc.16").short_of(&min));
        assert!(!NodeRuntime::named("1.0.0").short_of(&min));
        // A node from before runtimes were asked for is older than the asker.
        assert!(NodeRuntime::Before(v("1.0.0-rc.16")).short_of(&min));
        assert!(NodeRuntime::Before(v("1.0.0-rc.15")).short_of(&min));
        assert!(
            !NodeRuntime::Before(v("1.0.0")).short_of(&min),
            "older than 1.0.0 may still be rc.16 or newer"
        );
        // A development build names no version: judged as before.
        assert_eq!(NodeRuntime::named("test"), NodeRuntime::Unknown);
        assert!(!NodeRuntime::Unknown.short_of(&v("99.0.0")));
    }

    #[test]
    fn the_refusal_names_each_node_and_what_it_runs() {
        let runtimes = BTreeMap::from([
            (1, NodeRuntime::named("2.0.0")),
            (2, NodeRuntime::named("1.0.0")),
            (3, NodeRuntime::Before(v("1.5.0"))),
            (4, NodeRuntime::Unknown),
        ]);
        let min = v("1.5.0");
        let short = short_of(&[1, 2, 3, 4, 5], &runtimes, &min);
        assert_eq!(
            short,
            vec![
                json!({ "node": 2, "runtime": "1.0.0" }),
                json!({ "node": 3, "runtime": { "before": "1.5.0" } }),
            ]
        );
        let e = refusal("release 2.0.0", &min, short);
        assert_eq!(e.code, RUNTIME_TOO_OLD);
        assert!(
            e.message.contains("node 2 runs 1.0.0")
                && e.message.contains("node 3 runs a release older than 1.5.0"),
            "{}",
            e.message
        );
    }
}
