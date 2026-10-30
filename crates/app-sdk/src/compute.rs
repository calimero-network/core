//! What a `#[app::compute]` method sees of its run: its id, the files it may read and a way
//! to report progress. Outside a compute run there is no run and no input.

use crate::files::FileId;

#[cfg(target_arch = "wasm32")]
mod backend {
    use crate::bindings::calimero::app::compute;

    pub fn run_id() -> Option<String> {
        compute::run_id()
    }

    pub fn inputs() -> Vec<String> {
        compute::inputs()
    }

    pub fn progress(percent: u8, step: &str) {
        compute::progress(percent, step);
    }
}

#[cfg(not(target_arch = "wasm32"))]
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
pub fn inputs() -> Vec<FileId> {
    backend::inputs().into_iter().map(FileId::from).collect()
}

/// Reports progress: `percent` is capped at 100 and `step` cut at 128 bytes. The node shows the
/// last report; it is never logged.
pub fn progress(percent: u8, step: &str) {
    backend::progress(percent, step);
}
