mod check;
mod compile;
mod model;

pub use check::{check, priced_cost, run_cost, worst_case_cost, worst_case_fuel};
pub use compile::*;
pub use model::*;

/// Deterministic bytes (RFC 8785) for a pipeline: equal pipelines give equal bytes.
pub fn canonical_json(p: &Pipeline) -> Vec<u8> {
    serde_json_canonicalizer::to_vec(p).expect("a Pipeline always serializes")
}
