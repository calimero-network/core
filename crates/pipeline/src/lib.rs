//! The pipeline YAML language: compiled to a [`Pipeline`] and its canonical JSON
//! ([`canonical_json`]), checked against a cluster's modules and pools, and bounded by its
//! worst-case fuel and cost ([`worst_case_fuel`], [`worst_case_cost`], [`estimate`]).
//! Pure: no node dependency, so the node, `meroctl` and tests compile a pipeline alike.
//!
//! Spec: [Pipeline runs](https://calimero-network.github.io/core/protocol/compute-actions/#pipeline-runs),
//! [Sealed files and pipelines](https://calimero-network.github.io/core/build/guides/files-and-pipelines/).
//! Crate guide: [`crates/pipeline/AGENTS.md`](https://github.com/calimero-network/core/blob/master/crates/pipeline/AGENTS.md).

mod check;
mod compile;
mod model;

pub use check::{
    check, estimate, priced_cost, run_cost, worst_case_cost, worst_case_fuel, Estimate,
    StepEstimate,
};
pub use compile::*;
pub use model::*;

/// Deterministic bytes (RFC 8785) for a pipeline: equal pipelines give equal bytes.
pub fn canonical_json(p: &Pipeline) -> Vec<u8> {
    serde_json_canonicalizer::to_vec(p).expect("a Pipeline always serializes")
}
