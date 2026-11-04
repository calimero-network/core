//! What a `#[app::compute]` method sees of its run: its id, the files it may read and a way
//! to report progress. Outside a compute run there is no run and no input.

use crate::blob::BlobRef;

mod backend {
    pub fn run_id() -> Option<String> {
        None
    }

    pub fn inputs() -> Vec<String> {
        Vec::new()
    }

    pub fn progress(_percent: u8, _step: &str) {}
}

/// The id of this run; none outside a compute run.
pub fn run_id() -> Option<String> {
    backend::run_id()
}

/// The files this run may open, in the order the app gave them.
pub fn inputs() -> Vec<BlobRef> {
    backend::inputs()
        .iter()
        .filter_map(|id| BlobRef::parse(id))
        .collect()
}

/// Reports progress: `percent` is capped at 100 and `step` cut at 128 bytes. The node shows the
/// last report; it is never logged.
pub fn progress(percent: u8, step: &str) {
    backend::progress(percent, step);
}
