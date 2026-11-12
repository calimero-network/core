//! The live, metadata-only events of a pipeline run, shared by the node that sends them and the
//! client that reads them. Each is a typed struct, so a new record field never reaches a stream.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A run's own state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunEvent {
    pub run: String,
    pub pipeline: String,
    pub state: String,
    pub start_ts_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_ts_ms: Option<u64>,
    /// Decimal string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_so_far: Option<String>,
    /// Decimal string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost: Option<String>,
}

/// One step's progress.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StepEvent {
    pub run: String,
    pub step: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u32>,
    pub finished: u32,
    pub failed_items: u32,
    pub cached_items: u32,
    /// The stable failure code, never text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The first event: the run and every step as they are.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Snapshot {
    pub run: RunEvent,
    pub steps: Vec<StepEvent>,
}

/// One frame of the stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PipelineEvent {
    Snapshot(Snapshot),
    Run(RunEvent),
    Step(StepEvent),
    /// The run left `running`; the stream closes after it.
    End(RunEvent),
}

impl PipelineEvent {
    /// The SSE event name.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Snapshot(_) => "snapshot",
            Self::Run(_) => "run",
            Self::Step(_) => "step",
            Self::End(_) => "end",
        }
    }

    /// The SSE data: the typed struct as JSON.
    #[must_use]
    pub fn data(&self) -> String {
        let v = match self {
            Self::Snapshot(s) => serde_json::to_string(s),
            Self::Run(r) | Self::End(r) => serde_json::to_string(r),
            Self::Step(s) => serde_json::to_string(s),
        };
        v.unwrap_or_default()
    }

    /// Reads one frame; `None` for a name or shape this client does not know.
    #[must_use]
    pub fn parse(name: &str, data: &Value) -> Option<Self> {
        let d = data.clone();
        Some(match name {
            "snapshot" => Self::Snapshot(serde_json::from_value(d).ok()?),
            "run" => Self::Run(serde_json::from_value(d).ok()?),
            "step" => Self::Step(serde_json::from_value(d).ok()?),
            "end" => Self::End(serde_json::from_value(d).ok()?),
            _ => return None,
        })
    }
}

/// A run and its steps at one moment, what two views are compared by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunProgress {
    pub run: RunEvent,
    pub steps: BTreeMap<String, StepEvent>,
}

impl RunProgress {
    /// Reads the run view's JSON, taking only the fields an event carries.
    #[must_use]
    pub fn from_view(v: &Value) -> Option<Self> {
        let id = v["id"].as_str()?;
        let text = |x: &Value| x.as_str().map(str::to_owned);
        let run = RunEvent {
            run: id.to_owned(),
            pipeline: text(&v["pipeline"])?,
            state: text(&v["state"])?,
            start_ts_ms: v["startTsMs"].as_u64()?,
            end_ts_ms: v["endTsMs"].as_u64(),
            cost_so_far: text(&v["costSoFar"]),
            max_cost: text(&v["cost"]["maxCost"]),
        };
        let steps = v["steps"]
            .as_object()?
            .iter()
            .map(|(name, s)| {
                let ev = StepEvent {
                    run: id.to_owned(),
                    step: name.clone(),
                    status: text(&s["status"])?,
                    total: s["fan"]["total"]
                        .as_u64()
                        .and_then(|n| u32::try_from(n).ok())
                        .filter(|n| *n > 0),
                    finished: u32::try_from(s["finished"].as_u64()?).ok()?,
                    failed_items: u32::try_from(s["failedItems"].as_u64()?).ok()?,
                    cached_items: u32::try_from(s["cachedItems"].as_u64().unwrap_or(0)).ok()?,
                    reason: text(&s["reason"]),
                };
                Some((name.clone(), ev))
            })
            .collect::<Option<_>>()?;
        Some(Self { run, steps })
    }

    /// The first event of a stream.
    #[must_use]
    pub fn snapshot(&self) -> PipelineEvent {
        PipelineEvent::Snapshot(Snapshot {
            run: self.run.clone(),
            steps: self.steps.values().cloned().collect(),
        })
    }

    /// What changed from `prev` to `self`: a `step` event per changed step, a `run` event when
    /// the run changed, and `end` instead of it once the run is not running.
    #[must_use]
    pub fn diff(&self, prev: &Self) -> Vec<PipelineEvent> {
        let mut out: Vec<PipelineEvent> = self
            .steps
            .iter()
            .filter(|(k, s)| prev.steps.get(*k) != Some(s))
            .map(|(_, s)| PipelineEvent::Step(s.clone()))
            .collect();
        if self.ended() {
            out.push(PipelineEvent::End(self.run.clone()));
        } else if self.run != prev.run {
            out.push(PipelineEvent::Run(self.run.clone()));
        }
        out
    }

    /// Whether the run has left `running`.
    #[must_use]
    pub fn ended(&self) -> bool {
        self.run.state != "running"
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn view(state: &str, status: &str, finished: u32) -> Value {
        json!({
            "id": "r1", "pipeline": "p", "state": state, "startTsMs": 5,
            "owner": "secret-owner", "params": { "k": "param-value" },
            "inputs": { "docs": ["fileid"] },
            "cost": { "maxCost": "9", "worstCase": "99" }, "costSoFar": "3",
            "steps": { "a": {
                "status": status, "finished": finished, "failedItems": 0, "cachedItems": 1,
                "fan": { "total": 2, "items": 2, "batch": 1, "allowed": 0 },
                "reason": "COST_CAP", "result": { "leak": 1 },
                "outputs": [{ "id": "outid", "sha256": "h" }], "results": [1]
            } }
        })
    }

    fn keys(v: &Value) -> Vec<String> {
        let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
        k.sort();
        k
    }

    #[test]
    fn each_event_has_exactly_its_documented_keys() {
        let p = RunProgress::from_view(&view("running", "running", 1)).unwrap();
        let run: Value = serde_json::from_str(&PipelineEvent::Run(p.run.clone()).data()).unwrap();
        assert_eq!(
            keys(&run),
            [
                "costSoFar",
                "maxCost",
                "pipeline",
                "run",
                "startTsMs",
                "state"
            ]
        );
        let step = &p.steps["a"];
        let step: Value = serde_json::from_str(&PipelineEvent::Step(step.clone()).data()).unwrap();
        assert_eq!(
            keys(&step),
            [
                "cachedItems",
                "failedItems",
                "finished",
                "reason",
                "run",
                "status",
                "step",
                "total"
            ]
        );
        let snap: Value = serde_json::from_str(&p.snapshot().data()).unwrap();
        assert_eq!(keys(&snap), ["run", "steps"]);
        for banned in [
            "leak",
            "outid",
            "fileid",
            "param-value",
            "secret-owner",
            "worstCase",
        ] {
            assert!(!snap.to_string().contains(banned), "{banned} leaked");
        }
    }

    #[test]
    fn a_diff_names_what_moved_and_nothing_for_an_unchanged_record() {
        let a = RunProgress::from_view(&view("running", "running", 1)).unwrap();
        assert!(a.diff(&a).is_empty());
        let b = RunProgress::from_view(&view("running", "running", 2)).unwrap();
        assert!(matches!(&b.diff(&a)[..], [PipelineEvent::Step(s)] if s.finished == 2));
        let c = RunProgress::from_view(&view("succeeded", "succeeded", 2)).unwrap();
        let d = c.diff(&b);
        assert!(
            matches!(&d[..], [PipelineEvent::Step(_), PipelineEvent::End(r)] if r.state == "succeeded")
        );
        let mut cost = view("running", "running", 1);
        cost["costSoFar"] = json!("4");
        let e = RunProgress::from_view(&cost).unwrap().diff(&a);
        assert!(matches!(&e[..], [PipelineEvent::Run(r)] if r.cost_so_far.as_deref() == Some("4")));
    }
}
