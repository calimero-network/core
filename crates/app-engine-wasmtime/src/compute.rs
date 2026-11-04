//! The `calimero:app/compute` world: what a job node links for a compute component.
//!
//! The interfaces it shares with the app world (`types`, `files`) reuse `bindings.rs`'s
//! implementations; only `job` is new. No state, xcall or `wasi:http` is linked, so a component
//! asking for one fails to prepare.

use wasmtime::component::HasSelf;
use wasmtime::component::Linker;
use wasmtime::Result;

use crate::bindings::calimero::app::{files, types};
use crate::errors::denied;
use crate::StoreData;

wasmtime::component::bindgen!({
    // The same directory as `calimero_app_wit::WIT_DIR`; bindgen needs a literal.
    path: "../app-wit/wit",
    world: "calimero:app/compute",
    imports: { default: trappable },
    with: {
        "wasi:io": wasmtime_wasi::p2::bindings::sync::io,
        "wasi:clocks": wasmtime_wasi::p2::bindings::clocks,
        "wasi:random": wasmtime_wasi::p2::bindings::random,
        "wasi:cli": wasmtime_wasi::p2::bindings::cli,
        "calimero:app/types": crate::bindings::calimero::app::types,
        "calimero:app/files": crate::bindings::calimero::app::files,
    },
    require_store_data_send: true,
});

use self::calimero::app::job;

/// Links the compute world's `calimero:app` interfaces onto `StoreData`.
pub(crate) fn add_to_linker(linker: &mut Linker<StoreData>) -> Result<()> {
    types::add_to_linker::<StoreData, HasSelf<StoreData>>(linker, |d| d)?;
    files::add_to_linker::<StoreData, HasSelf<StoreData>>(linker, |d| d)?;
    job::add_to_linker::<StoreData, HasSelf<StoreData>>(linker, |d| d)?;
    Ok(())
}

impl job::Host for StoreData {
    fn run_id(&mut self) -> Result<Option<String>> {
        Ok(self.host.compute_run_id())
    }

    fn inputs(&mut self) -> Result<Vec<String>> {
        Ok(self.host.compute_inputs())
    }

    fn progress(&mut self, percent: u8, step: String) -> Result<()> {
        self.host.compute_progress(percent, &step).map_err(denied)
    }
}
