//! The `calimero:app/sandbox-run` world: what a sandbox job node links for a sandbox
//! component, the compute world plus the `sandbox` interface.
//!
//! The interfaces it shares with the compute world (`types`, `files`, `job`) reuse their
//! implementations; `sandbox` maps onto `HostCore::sandbox_*`, which reach the job node's Linux
//! sandbox through the run's `SandboxHost`.

use calimero_app_host::SandboxExec;
use wasmtime::component::HasSelf;
use wasmtime::component::Linker;
use wasmtime::Result;

use crate::bindings::calimero::app::{files, types};
use crate::compute::calimero::app::job;
use crate::errors::denied;
use crate::StoreData;

wasmtime::component::bindgen!({
    // The same directory as `calimero_app_wit::WIT_DIR`; bindgen needs a literal.
    path: "../app-wit/wit",
    world: "calimero:app/sandbox-run",
    imports: { default: trappable },
    with: {
        "wasi:io": wasmtime_wasi::p2::bindings::sync::io,
        "wasi:clocks": wasmtime_wasi::p2::bindings::clocks,
        "wasi:random": wasmtime_wasi::p2::bindings::random,
        "wasi:cli": wasmtime_wasi::p2::bindings::cli,
        "calimero:app/types": crate::bindings::calimero::app::types,
        "calimero:app/files": crate::bindings::calimero::app::files,
        "calimero:app/job": crate::compute::calimero::app::job,
    },
    require_store_data_send: true,
});

use self::calimero::app::sandbox;

/// Links the sandbox world's `calimero:app` interfaces onto `StoreData`.
pub(crate) fn add_to_linker(linker: &mut Linker<StoreData>) -> Result<()> {
    types::add_to_linker::<StoreData, HasSelf<StoreData>>(linker, |d| d)?;
    files::add_to_linker::<StoreData, HasSelf<StoreData>>(linker, |d| d)?;
    job::add_to_linker::<StoreData, HasSelf<StoreData>>(linker, |d| d)?;
    sandbox::add_to_linker::<StoreData, HasSelf<StoreData>>(linker, |d| d)?;
    Ok(())
}

impl sandbox::Host for StoreData {
    fn exec(
        &mut self,
        req: sandbox::ExecRequest,
    ) -> Result<core::result::Result<sandbox::ExecResult, String>> {
        let req = SandboxExec {
            argv: req.argv,
            env: req.env,
            cwd: req.cwd,
            stdin: req.stdin,
            timeout_ms: req.timeout_ms,
        };
        let answer = self.host.sandbox_exec(req).map_err(denied)?;
        Ok(answer.map(|r| sandbox::ExecResult {
            exit_code: r.exit_code,
            stdout: r.stdout,
            stderr: r.stderr,
            truncated: r.truncated,
            timed_out: r.timed_out,
            wall_us: r.wall_us,
        }))
    }

    fn write(&mut self, path: String, bytes: Vec<u8>) -> Result<core::result::Result<(), String>> {
        self.host.sandbox_write(&path, &bytes).map_err(denied)
    }

    fn read(&mut self, path: String, max: u32) -> Result<core::result::Result<Vec<u8>, String>> {
        self.host.sandbox_read(&path, max).map_err(denied)
    }

    fn collect(&mut self, path: String) -> Result<core::result::Result<String, String>> {
        self.host.sandbox_collect(&path).map_err(denied)
    }
}

/// Shadows the sandbox world's `job` and `sandbox` functions with their charged versions
/// (`fuel.rs`); `files` is `bindings::charge_files`.
pub(crate) fn charge_calls(linker: &mut Linker<StoreData>) -> Result<()> {
    use crate::fuel::{calimero, charged};
    crate::compute::charge_calls(linker)?;
    let mut i = calimero(linker, "sandbox")?;
    charged!(i, "exec", |d, req: sandbox::ExecRequest| {
        sandbox::Host::exec(d, req)
    });
    charged!(i, "write", |d, path: String, bytes: Vec<u8>| {
        sandbox::Host::write(d, path, bytes)
    });
    charged!(i, "read", |d, path: String, max: u32| sandbox::Host::read(
        d, path, max
    ));
    charged!(i, "collect", |d, path: String| sandbox::Host::collect(
        d, path
    ));
    Ok(())
}
